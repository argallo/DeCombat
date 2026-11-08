//! The community directory document: servers plus the known-mods list. One static JSON file
//! (`community.json`), read by the landing page and the in-client browser alike.
//! SPEC: HOSTING_AND_MODS.md §5, §8.2 (known-mods list, verified badge), §8.3 (static JSON on GitHub Pages).
//!
//! `directory/schema.json` is the language-neutral description; this module is the Rust validator, and the
//! tests keep the two consistent.

use crate::Issue;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub use crate::schema::ModRef;

/// Current document version.
pub const SCHEMA_VERSION: u32 = 1;

/// Region codes used by directory entries and `directory.region`.
pub const REGIONS: &[&str] = &["na", "sa", "eu", "as", "oc", "af", "lan"];

/// Server policies. SPEC: HOSTING_AND_MODS.md §8.1.
pub const POLICIES: &[&str] = &["vanilla", "modded"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Community {
    pub schema_version: u32,
    /// UTC timestamp `YYYY-MM-DDTHH:MM:SSZ` of the last edit.
    pub updated: String,
    pub servers: Vec<ServerEntry>,
    pub known_mods: Vec<KnownMod>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Players {
    pub current: u32,
    pub max: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerEntry {
    /// Lowercase slug, unique within the document.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub region: String,
    /// `host:port` of the lobby, as the client connects.
    pub address: String,
    /// "vanilla" or "modded".
    pub policy: String,
    #[serde(default)]
    pub client_visual_allowed: bool,
    /// Last known occupancy, if the list maintainer records one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub players: Option<Players>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub website: Option<String>,
    /// CLI-1616: the SHA-256 fingerprint of the lobby's TLS certificate (64 hex digits, colons allowed); the
    /// in-client browser pins it for the login.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    /// Ordered mod set; empty for vanilla.
    #[serde(default)]
    pub mods: Vec<ModRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnownMod {
    pub id: String,
    pub name: String,
    pub version: String,
    pub sha256: String,
    /// Source repository URL (https).
    pub source_repo: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    /// Declared capabilities (content domains, hooks, storage).
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_url: Option<String>,
    /// LG-315 (amended): the GitHub release page the zip was published on, when it came from a pack repository's
    /// release. Clients before 2026-11-09 refuse a document that carries it (their row struct refuses unknown
    /// fields), so the shipped community.json leaves it out until those clients are gone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
}

/// A certificate fingerprint: 64 hex digits, optionally as 32 colon-separated pairs.
pub fn is_fingerprint(s: &str) -> bool {
    let hex: String = s.chars().filter(|c| *c != ':').collect();
    hex.len() == 64
        && hex.chars().all(|c| c.is_ascii_hexdigit())
        && (!s.contains(':') || s.split(':').all(|p| p.len() == 2))
}

/// Badge shown next to a mod on a server's details pane. SPEC: HOSTING_AND_MODS.md §8.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Badge {
    /// `(id, version, sha256)` is in the known-mods list.
    Verified,
    /// Not listed: explicit per-mod opt-in on first join.
    Unverified,
}

/// Parse and validate in one step. Parse failures come back as a single error issue.
pub fn load_community(json: &str) -> Result<Community, Vec<Issue>> {
    serde_json::from_str::<Community>(json).map_err(|e| {
        vec![Issue::error(
            "$",
            format!("not a valid community document: {e}"),
        )]
    })
}

/// Badge for each mod of `server`, in declared order.
pub fn badges(server: &ServerEntry, doc: &Community) -> Vec<(ModRef, Badge)> {
    server
        .mods
        .iter()
        .map(|m| {
            let known = doc
                .known_mods
                .iter()
                .any(|k| k.id == m.id && k.version == m.version && k.sha256 == m.sha256);
            (
                m.clone(),
                if known {
                    Badge::Verified
                } else {
                    Badge::Unverified
                },
            )
        })
        .collect()
}

/// Validate a whole document; returns every problem found (empty vec = valid).
pub fn validate_community(doc: &Community) -> Vec<Issue> {
    let mut out = Vec::new();
    if doc.schema_version != SCHEMA_VERSION {
        out.push(Issue::error(
            "schema_version",
            format!("expected {SCHEMA_VERSION}, found {}", doc.schema_version),
        ));
    }
    if !is_utc_stamp(&doc.updated) {
        out.push(Issue::error(
            "updated",
            format!(
                "'{}' is not a UTC timestamp like 2026-10-24T12:00:00Z",
                doc.updated
            ),
        ));
    }

    let mut ids = BTreeSet::new();
    for (i, s) in doc.servers.iter().enumerate() {
        let p = format!("servers[{i}]");
        if !is_slug(&s.id) {
            out.push(Issue::error(
                format!("{p}.id"),
                format!("'{}' must be 3-40 chars of a-z, 0-9 and '-'", s.id),
            ));
        } else if !ids.insert(s.id.as_str()) {
            out.push(Issue::error(
                format!("{p}.id"),
                format!("duplicate server id '{}'", s.id),
            ));
        }
        if s.name.trim().is_empty() || s.name.chars().count() > 48 {
            out.push(Issue::error(format!("{p}.name"), "must be 1-48 characters"));
        }
        if s.description.chars().count() > 400 {
            out.push(Issue::error(
                format!("{p}.description"),
                "must be at most 400 characters",
            ));
        }
        if !REGIONS.contains(&s.region.as_str()) {
            out.push(Issue::error(
                format!("{p}.region"),
                format!("'{}' is not one of {}", s.region, REGIONS.join(", ")),
            ));
        }
        if let Err(e) = check_host_port(&s.address) {
            out.push(Issue::error(format!("{p}.address"), e));
        }
        if let Some(pl) = &s.players {
            if pl.max == 0 || pl.current > pl.max {
                out.push(Issue::error(
                    format!("{p}.players"),
                    format!("current {} must be <= max {} (max > 0)", pl.current, pl.max),
                ));
            }
        }
        if let Some(w) = &s.website {
            if !is_http_url(w) {
                out.push(Issue::error(
                    format!("{p}.website"),
                    "must be an http(s) URL",
                ));
            }
        }
        if let Some(fp) = &s.fingerprint {
            if !is_fingerprint(fp) {
                out.push(Issue::error(
                    format!("{p}.fingerprint"),
                    "must be 64 hex digits (colons between the pairs allowed)",
                ));
            }
        }
        match s.policy.as_str() {
            "vanilla" => {
                if !s.mods.is_empty() {
                    out.push(Issue::error(
                        format!("{p}.mods"),
                        "a vanilla server has no mods at all (HOSTING_AND_MODS.md §8.1)",
                    ));
                }
                if s.client_visual_allowed {
                    out.push(Issue::error(
                        format!("{p}.client_visual_allowed"),
                        "must be false on a vanilla server",
                    ));
                }
            }
            "modded" => {
                if s.mods.is_empty() {
                    out.push(Issue::error(
                        format!("{p}.mods"),
                        "a modded server must list at least one mod",
                    ));
                }
            }
            other => out.push(Issue::error(
                format!("{p}.policy"),
                format!("'{other}' is not one of vanilla, modded"),
            )),
        }
        let mut seen = BTreeSet::new();
        for (j, m) in s.mods.iter().enumerate() {
            out.extend(check_mod_ref(&format!("{p}.mods[{j}]"), m));
            if !seen.insert(m.id.as_str()) {
                out.push(Issue::error(
                    format!("{p}.mods[{j}].id"),
                    format!("mod '{}' listed twice", m.id),
                ));
            }
            // A server may not claim a known (id, version) with different bytes.
            for k in doc
                .known_mods
                .iter()
                .filter(|k| k.id == m.id && k.version == m.version)
            {
                if k.sha256 != m.sha256 {
                    out.push(Issue::error(
                        format!("{p}.mods[{j}].sha256"),
                        format!(
                            "mod {} {} is in known_mods with a different sha256",
                            m.id, m.version
                        ),
                    ));
                }
            }
        }
    }

    let mut keys: BTreeMap<(&str, &str), &str> = BTreeMap::new();
    for (i, k) in doc.known_mods.iter().enumerate() {
        let p = format!("known_mods[{i}]");
        out.extend(check_mod_ref(
            &p,
            &ModRef {
                id: k.id.clone(),
                version: k.version.clone(),
                sha256: k.sha256.clone(),
            },
        ));
        if k.name.trim().is_empty() {
            out.push(Issue::error(format!("{p}.name"), "must not be empty"));
        }
        if !is_http_url(&k.source_repo) || !k.source_repo.starts_with("https://") {
            out.push(Issue::error(
                format!("{p}.source_repo"),
                "must be an https URL",
            ));
        }
        if let Some(u) = &k.manifest_url {
            if !u.starts_with("https://") {
                out.push(Issue::error(
                    format!("{p}.manifest_url"),
                    "must be an https URL",
                ));
            }
        }
        if let Some(r) = &k.release {
            if !crate::schema::is_release_page(r) {
                out.push(Issue::error(
                    format!("{p}.release"),
                    "must be a GitHub release page, https://github.com/<owner>/<repo>/releases/tag/<tag> (a tag, never a branch)",
                ));
            }
        }
        if k.capabilities.iter().any(|c| c.trim().is_empty()) {
            out.push(Issue::error(
                format!("{p}.capabilities"),
                "entries must not be empty",
            ));
        }
        if let Some(prev) = keys.insert((k.id.as_str(), k.version.as_str()), k.sha256.as_str()) {
            let msg = if prev == k.sha256 {
                "duplicate entry"
            } else {
                "same id and version with two different hashes"
            };
            out.push(Issue::error(p, format!("{} {}: {msg}", k.id, k.version)));
        }
    }
    out
}

/// Shared by the server.toml mod-set check and the directory validator.
pub(crate) fn check_mod_ref(path: &str, m: &ModRef) -> Vec<Issue> {
    let mut out = Vec::new();
    if !is_mod_id(&m.id) {
        out.push(Issue::error(
            format!("{path}.id"),
            format!("'{}' must be 1-64 chars of a-z, 0-9, '_', '-' or '.'", m.id),
        ));
    }
    if m.version.is_empty() || m.version.len() > 32 || m.version.chars().any(|c| c.is_whitespace())
    {
        out.push(Issue::error(
            format!("{path}.version"),
            "must be 1-32 characters without spaces",
        ));
    }
    if !is_sha256(&m.sha256) {
        out.push(Issue::error(
            format!("{path}.sha256"),
            "must be 64 lowercase hex digits",
        ));
    }
    out
}

pub(crate) fn is_sha256(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(crate) fn is_mod_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-' | b'.')
        })
}

fn is_slug(s: &str) -> bool {
    (3..=40).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

pub(crate) fn is_http_url(s: &str) -> bool {
    let rest = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"));
    match rest {
        Some(r) => !r.is_empty() && !r.starts_with('/') && !s.chars().any(|c| c.is_whitespace()),
        None => false,
    }
}

fn is_utc_stamp(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 20 {
        return false;
    }
    let digit = |i: usize| b[i].is_ascii_digit();
    let shape = [
        (4, b'-'),
        (7, b'-'),
        (10, b'T'),
        (13, b':'),
        (16, b':'),
        (19, b'Z'),
    ];
    shape.iter().all(|&(i, c)| b[i] == c)
        && [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18]
            .iter()
            .all(|&i| digit(i))
        && matches!(
            &s[5..7],
            "01" | "02" | "03" | "04" | "05" | "06" | "07" | "08" | "09" | "10" | "11" | "12"
        )
        && (1..=31).contains(&s[8..10].parse::<u32>().unwrap_or(0))
        && s[11..13].parse::<u32>().unwrap_or(99) < 24
        && s[14..16].parse::<u32>().unwrap_or(99) < 60
        && s[17..19].parse::<u32>().unwrap_or(99) < 60
}

/// `host:port`, host = DNS name, IPv4 or `[IPv6]`; port 1..=65535.
pub(crate) fn check_host_port(s: &str) -> Result<(), String> {
    let (host, port) = s
        .rsplit_once(':')
        .ok_or_else(|| format!("'{s}' must look like host:port"))?;
    match port.parse::<u32>() {
        Ok(p) if (1..=65535).contains(&p) => {}
        _ => return Err(format!("'{port}' is not a port in 1..=65535")),
    }
    let ok_host = if host.starts_with('[') && host.ends_with(']') {
        host[1..host.len() - 1]
            .parse::<std::net::Ipv6Addr>()
            .is_ok()
    } else {
        !host.is_empty()
            && host.len() <= 253
            && !host.starts_with('.')
            && !host.ends_with('.')
            && host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    };
    if ok_host {
        Ok(())
    } else {
        Err(format!("'{host}' is not a valid host name or address"))
    }
}
