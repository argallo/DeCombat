//! dc-config: `server.toml` schema, env overrides (`DC_<SECTION>__<KEY>`), CLI overrides, validation, and the
//! community directory validator. SPEC: HOSTING_AND_MODS.md §2 (one `server.toml` plus env overrides, sane
//! defaults), §5 and §8 (directory, mod-set declaration, vanilla policy).
//!
//! Precedence, lowest to highest: built-in defaults, `server.toml`, `DC_*` environment, command-line overrides.
//!
//! `modes` holds the DeCombat mode-name vocabulary (`modes.playable`, map records, spawn filters): the name <->
//! ROM class mapping and the `serves` rule, shared by the lobby, the game server and the mod runtime.

mod check;
pub mod directory;
pub mod modes;
pub mod vanilla_plus;
pub mod schema;

pub use check::{
    check, public_loopback_advertise, CheckOptions, Ports, MAX_CHANNELS, MIN_HOST_TOKEN,
};
pub use schema::*;

use std::collections::BTreeSet;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// One validation finding with the config path (`dserver.port_range`) or JSON path it concerns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub severity: Severity,
    pub path: String,
    pub message: String,
}

impl Issue {
    pub fn error(path: impl Into<String>, message: impl Into<String>) -> Issue {
        Issue {
            severity: Severity::Error,
            path: path.into(),
            message: message.into(),
        }
    }
    pub fn warning(path: impl Into<String>, message: impl Into<String>) -> Issue {
        Issue {
            severity: Severity::Warning,
            path: path.into(),
            message: message.into(),
        }
    }
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tag = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        write!(f, "{tag}: {}: {}", self.path, self.message)
    }
}

/// True if any issue is an error.
pub fn has_errors(issues: &[Issue]) -> bool {
    issues.iter().any(Issue::is_error)
}

/// Command-line overrides (`--rom`, `--public`, ...). `None` leaves the lower layers alone.
#[derive(Debug, Clone, Default)]
pub struct CliOverrides {
    pub rom: Option<String>,
    pub public: bool,
    pub bind: Option<String>,
    pub metrics: bool,
    pub db: Option<String>,
    pub mods_dir: Option<String>,
}

/// The built-in example `server.toml`, with comments. Kept valid by tests.
pub const EXAMPLE_SERVER_TOML: &str = include_str!("../server.example.toml");

/// Parse TOML text (defaults fill every missing key).
pub fn parse_toml(text: &str) -> Result<Config, Vec<Issue>> {
    toml::from_str::<Config>(text)
        .map_err(|e| vec![Issue::error("server.toml", e.message().to_string())])
}

/// The environment prefix for overrides. `DC_SERVER__PUBLIC=true`, `DC_DSERVER__TICK_HZ=30`,
/// `DC_ECONOMY__EXP__KILL=5`. Double underscore separates path segments (key names contain single ones).
/// Variables without a `__` (for example the build variable `DC_TARGET`) are not config and are ignored.
pub const ENV_PREFIX: &str = "DC_";

/// Apply `DC_<SECTION>__<KEY>` overrides from `vars` on top of `cfg`.
pub fn apply_env<I>(cfg: &Config, vars: I) -> Result<Config, Vec<Issue>>
where
    I: IntoIterator<Item = (String, String)>,
{
    let mut tree =
        toml::Value::try_from(cfg).map_err(|e| vec![Issue::error("config", e.to_string())])?;
    let mut issues = Vec::new();
    let mut vars: Vec<(String, String)> = vars
        .into_iter()
        .filter(|(k, _)| k.starts_with(ENV_PREFIX) && k[ENV_PREFIX.len()..].contains("__"))
        .collect();
    vars.sort(); // deterministic order
    for (name, raw) in vars {
        let segs: Vec<&str> = name[ENV_PREFIX.len()..].split("__").collect();
        if let Err(e) = set_path(&mut tree, &segs, &raw) {
            issues.push(Issue::error(name.clone(), e));
        }
    }
    if !issues.is_empty() {
        return Err(issues);
    }
    tree.try_into::<Config>().map_err(|e| {
        vec![Issue::error(
            "environment",
            format!("override produced an invalid config: {}", e.message()),
        )]
    })
}

/// Paths whose children are free-form: unknown segments are inserted instead of rejected.
/// The bool says whether segment case is kept (map keys such as mode keys) or lowercased.
const FREE_MAPS: &[(&str, bool)] = &[
    ("clan.mode_weight", true),
    ("shop.coupons", true),
    ("shop.coupon_uses", true),
    ("shop.price_override", true),
    ("evars.extra", true),
    ("room.round_secs", false),
    ("gm", false),
];

fn set_path(root: &mut toml::Value, segs: &[&str], raw: &str) -> Result<(), String> {
    let mut cur = root;
    let mut walked = String::new();
    for (i, seg) in segs.iter().enumerate() {
        let last = i + 1 == segs.len();
        let table = cur
            .as_table_mut()
            .ok_or_else(|| format!("'{walked}' is not a table, cannot set '{seg}' below it"))?;
        let existing = table.keys().find(|k| k.eq_ignore_ascii_case(seg)).cloned();
        let key = match existing {
            Some(k) => k,
            None => match FREE_MAPS.iter().find(|(p, _)| *p == walked) {
                Some((_, keep_case)) => {
                    let k = if *keep_case {
                        seg.to_string()
                    } else {
                        seg.to_ascii_lowercase()
                    };
                    if last {
                        table.insert(k.clone(), guess_value(raw));
                        return Ok(());
                    }
                    table.insert(k.clone(), toml::Value::Table(Default::default()));
                    k
                }
                None => {
                    let known: Vec<&str> = table.keys().map(|s| s.as_str()).collect();
                    let at = if walked.is_empty() {
                        "the top level".to_string()
                    } else {
                        format!("'{walked}'")
                    };
                    return Err(format!(
                        "unknown config key '{}' at {at}; known: {}",
                        seg.to_ascii_lowercase(),
                        known.join(", ")
                    ));
                }
            },
        };
        if !walked.is_empty() {
            walked.push('.');
        }
        walked.push_str(&key);
        let slot = table.get_mut(&key).expect("key just found or inserted");
        if last {
            *slot = coerce(slot, raw).map_err(|e| format!("{walked}: {e}"))?;
            return Ok(());
        }
        cur = slot;
    }
    Ok(())
}

fn guess_value(raw: &str) -> toml::Value {
    parse_toml_value(raw).unwrap_or_else(|| toml::Value::String(raw.to_string()))
}

fn parse_toml_value(raw: &str) -> Option<toml::Value> {
    let doc: toml::Table = toml::from_str(&format!("v = {raw}")).ok()?;
    doc.get("v").cloned()
}

/// Convert `raw` to the TOML type already in the tree.
fn coerce(old: &toml::Value, raw: &str) -> Result<toml::Value, String> {
    use toml::Value as V;
    let t = raw.trim();
    match old {
        V::Boolean(_) => match t.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Ok(V::Boolean(true)),
            "false" | "0" | "no" | "off" => Ok(V::Boolean(false)),
            _ => Err(format!("'{raw}' is not a boolean (true/false)")),
        },
        V::Integer(_) => t
            .parse::<i64>()
            .map(V::Integer)
            .map_err(|_| format!("'{raw}' is not an integer")),
        // `rom` is the one word a number-or-ROM key (`combat.armor_absorb`) accepts in place of a number; a plain
        // number key rejects it when the config is rebuilt
        V::Float(_) if t.eq_ignore_ascii_case("rom") => Ok(V::String("rom".into())),
        V::Float(_) => t
            .parse::<f64>()
            .map(V::Float)
            .map_err(|_| format!("'{raw}' is not a number")),
        V::String(_) => Ok(V::String(raw.to_string())),
        V::Array(_) => match parse_toml_value(t) {
            Some(v @ V::Array(_)) => Ok(v),
            _ => Err(format!("'{raw}' is not a TOML array such as [1, 2]")),
        },
        V::Table(_) => Err("is a table; set one of its keys instead".to_string()),
        V::Datetime(_) => Ok(V::String(raw.to_string())),
    }
}

/// Apply CLI overrides (highest precedence).
pub fn apply_cli(cfg: &mut Config, cli: &CliOverrides) {
    if let Some(r) = &cli.rom {
        cfg.paths.rom = r.clone();
    }
    if cli.public {
        cfg.server.public = true;
    }
    if let Some(b) = &cli.bind {
        cfg.server.bind = b.clone();
    }
    if cli.metrics {
        cfg.metrics.enabled = true;
    }
    if let Some(d) = &cli.db {
        cfg.paths.db = d.clone();
    }
    if let Some(m) = &cli.mods_dir {
        cfg.paths.mods_dir = m.clone();
    }
}

/// Which config paths the operator set explicitly in the file or the environment. A value equal to the built-in
/// default is indistinguishable from "unset" in a [`Config`], so a consumer that has other layers beneath
/// `server.toml` (the lobby's `lobby.json`) asks this before letting a config value win.
///
/// Paths are lower-case dotted (`economy.exp.kill`). An array is one leaf; a table is walked, and an empty table
/// counts as set at its own path. [`Explicit::has`] is true for the leaf itself, for any prefix of it
/// (`economy.exp` when `economy.exp.kill` is set) and for any path below a leaf.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Explicit {
    leaves: BTreeSet<String>,
}

impl Explicit {
    /// Leaves present in a TOML document.
    pub fn from_toml(text: &str) -> Result<Explicit, Vec<Issue>> {
        let v: toml::Value = toml::from_str(text)
            .map_err(|e| vec![Issue::error("server.toml", e.message().to_string())])?;
        let mut e = Explicit::default();
        e.walk("", &v);
        Ok(e)
    }

    fn walk(&mut self, at: &str, v: &toml::Value) {
        match v {
            toml::Value::Table(t) if !t.is_empty() => {
                for (k, c) in t {
                    let p = if at.is_empty() {
                        k.to_ascii_lowercase()
                    } else {
                        format!("{at}.{}", k.to_ascii_lowercase())
                    };
                    self.walk(&p, c);
                }
            }
            _ => {
                if !at.is_empty() {
                    self.leaves.insert(at.to_string());
                }
            }
        }
    }

    /// Adds the paths named by `DC_<SECTION>__<KEY>` variables (same filter as [`apply_env`]).
    pub fn add_env<I: IntoIterator<Item = (String, String)>>(&mut self, vars: I) {
        for (k, _) in vars {
            if k.starts_with(ENV_PREFIX) && k[ENV_PREFIX.len()..].contains("__") {
                let p: Vec<String> = k[ENV_PREFIX.len()..]
                    .split("__")
                    .map(|s| s.to_ascii_lowercase())
                    .collect();
                self.leaves.insert(p.join("."));
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.leaves.is_empty()
    }

    /// True if `path` (any case) was set, or lies above or below something that was.
    pub fn has(&self, path: &str) -> bool {
        let p = path.to_ascii_lowercase();
        self.leaves
            .iter()
            .any(|l| *l == p || l.starts_with(&format!("{p}.")) || p.starts_with(&format!("{l}.")))
    }
}

/// [`load_layered`] plus the [`Explicit`] set of the file and the environment (not the CLI overrides).
pub fn load_layered_explicit<I>(
    file_text: Option<&str>,
    env: I,
    cli: &CliOverrides,
) -> Result<(Config, Explicit), Vec<Issue>>
where
    I: IntoIterator<Item = (String, String)> + Clone,
{
    let mut ex = match file_text {
        Some(t) => Explicit::from_toml(t)?,
        None => Explicit::default(),
    };
    ex.add_env(env.clone());
    Ok((load_layered(file_text, env, cli)?, ex))
}

/// Full layering: defaults < file text < env < CLI.
pub fn load_layered<I>(
    file_text: Option<&str>,
    env: I,
    cli: &CliOverrides,
) -> Result<Config, Vec<Issue>>
where
    I: IntoIterator<Item = (String, String)>,
{
    let base = match file_text {
        Some(t) => parse_toml(t)?,
        None => Config::default(),
    };
    let mut cfg = apply_env(&base, env)?;
    apply_cli(&mut cfg, cli);
    Ok(cfg)
}

impl Config {
    /// The key tree of this config (every key present), used by coverage tests and env overrides.
    pub fn to_value(&self) -> toml::Value {
        toml::Value::try_from(self).expect("config always serialises")
    }

    /// Address the listeners bind: `0.0.0.0` when `server.public`, else `server.bind`.
    pub fn effective_bind(&self) -> &str {
        if self.server.public {
            "0.0.0.0"
        } else {
            &self.server.bind
        }
    }

    /// Serialise as TOML (no comments; see [`EXAMPLE_SERVER_TOML`] for the documented form).
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("config always serialises")
    }
}
