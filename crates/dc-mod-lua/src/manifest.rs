use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::Path;

use crate::host::CONTENT_DOMAINS;

pub const ALLOWED_PERMISSIONS: &[&str] = &["network", "background", "engine_internals"];

/// Pure-Rust JSON value representation.
#[derive(Debug, Clone, PartialEq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<JsonValue>),
    Object(BTreeMap<String, JsonValue>),
}

impl JsonValue {
    pub fn parse(input: &str) -> Result<JsonValue, String> {
        let mut parser = JsonParser::new(input);
        let val = parser.parse_value()?;
        parser.skip_whitespace();
        if !parser.is_eof() {
            return Err(parser.error("Unexpected trailing characters after JSON value"));
        }
        Ok(val)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            JsonValue::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            JsonValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            JsonValue::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            JsonValue::Number(n)
                if n.fract() == 0.0 && *n >= i64::MIN as f64 && *n <= i64::MAX as f64 =>
            {
                Some(*n as i64)
            }
            _ => None,
        }
    }

    pub fn as_u32(&self) -> Option<u32> {
        self.as_i64().and_then(|v| u32::try_from(v).ok())
    }

    pub fn as_i32(&self) -> Option<i32> {
        self.as_i64().and_then(|v| i32::try_from(v).ok())
    }

    pub fn as_array(&self) -> Option<&[JsonValue]> {
        match self {
            JsonValue::Array(arr) => Some(arr.as_slice()),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&BTreeMap<String, JsonValue>> {
        match self {
            JsonValue::Object(obj) => Some(obj),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&JsonValue> {
        self.as_object().and_then(|obj| obj.get(key))
    }

    pub fn to_json_string(&self) -> String {
        let mut out = String::new();
        self.write_compact(&mut out);
        out
    }

    pub fn to_json_pretty(&self) -> String {
        let mut out = String::new();
        self.write_pretty(&mut out, 0);
        out
    }

    fn write_compact(&self, out: &mut String) {
        match self {
            JsonValue::Null => out.push_str("null"),
            JsonValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            JsonValue::Number(n) => write_json_number(*n, out),
            JsonValue::String(s) => write_json_string(s, out),
            JsonValue::Array(arr) => {
                out.push('[');
                for (i, item) in arr.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write_compact(out);
                }
                out.push(']');
            }
            JsonValue::Object(obj) => {
                out.push('{');
                for (i, (k, v)) in obj.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_json_string(k, out);
                    out.push(':');
                    v.write_compact(out);
                }
                out.push('}');
            }
        }
    }

    fn write_pretty(&self, out: &mut String, indent: usize) {
        let pad = "  ".repeat(indent);
        let inner_pad = "  ".repeat(indent + 1);
        match self {
            JsonValue::Null => out.push_str("null"),
            JsonValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            JsonValue::Number(n) => write_json_number(*n, out),
            JsonValue::String(s) => write_json_string(s, out),
            JsonValue::Array(arr) => {
                if arr.is_empty() {
                    out.push_str("[]");
                    return;
                }
                out.push_str("[\n");
                for (i, item) in arr.iter().enumerate() {
                    out.push_str(&inner_pad);
                    item.write_pretty(out, indent + 1);
                    if i + 1 < arr.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                out.push_str(&pad);
                out.push(']');
            }
            JsonValue::Object(obj) => {
                if obj.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push_str("{\n");
                for (i, (k, v)) in obj.iter().enumerate() {
                    out.push_str(&inner_pad);
                    write_json_string(k, out);
                    out.push_str(": ");
                    v.write_pretty(out, indent + 1);
                    if i + 1 < obj.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
                out.push_str(&pad);
                out.push('}');
            }
        }
    }
}

fn write_json_number(n: f64, out: &mut String) {
    if !n.is_finite() {
        out.push_str("null");
    } else if n.fract() == 0.0 && n.abs() < 1e15 {
        out.push_str(&format!("{}", n as i64));
    } else {
        out.push_str(&format!("{}", n));
    }
}

fn write_json_string(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

struct JsonParser<'a> {
    chars: Vec<char>,
    pos: usize,
    _raw: &'a str,
}

impl<'a> JsonParser<'a> {
    fn new(input: &'a str) -> Self {
        let stripped = input.strip_prefix('\u{feff}').unwrap_or(input);
        Self {
            chars: stripped.chars().collect(),
            pos: 0,
            _raw: stripped,
        }
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.chars.len()
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn next_char(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += 1;
        Some(ch)
    }

    fn skip_whitespace(&mut self) {
        while let Some(ch) = self.peek() {
            if ch == ' ' || ch == '\t' || ch == '\r' || ch == '\n' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn line_col(&self) -> (usize, usize) {
        let mut line = 1;
        let mut col = 1;
        for i in 0..self.pos.min(self.chars.len()) {
            if self.chars[i] == '\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
        }
        (line, col)
    }

    fn error(&self, msg: &str) -> String {
        let (line, col) = self.line_col();
        format!("JSON parse error at {}:{}: {}", line, col, msg)
    }

    fn parse_value(&mut self) -> Result<JsonValue, String> {
        self.skip_whitespace();
        let ch = self
            .peek()
            .ok_or_else(|| self.error("Unexpected end of JSON input"))?;
        match ch {
            '"' => self.read_json_string().map(JsonValue::String),
            '{' => self.parse_object(),
            '[' => self.parse_array(),
            't' => self.parse_literal("true", JsonValue::Bool(true)),
            'f' => self.parse_literal("false", JsonValue::Bool(false)),
            'n' => self.parse_literal("null", JsonValue::Null),
            '-' | '0'..='9' => self.parse_number().map(JsonValue::Number),
            _ => Err(self.error(&format!("Unexpected character '{}'", ch))),
        }
    }

    fn parse_literal(&mut self, expected: &str, value: JsonValue) -> Result<JsonValue, String> {
        for exp in expected.chars() {
            match self.next_char() {
                Some(c) if c == exp => {}
                _ => return Err(self.error(&format!("Expected '{}'", expected))),
            }
        }
        Ok(value)
    }

    fn read_json_string(&mut self) -> Result<String, String> {
        if self.next_char() != Some('"') {
            return Err(self.error("Expected '\"' to start string"));
        }
        let mut out = String::new();
        while let Some(ch) = self.next_char() {
            match ch {
                '"' => return Ok(out),
                '\\' => {
                    let esc = self
                        .next_char()
                        .ok_or_else(|| self.error("Unterminated escape sequence in string"))?;
                    match esc {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '/' => out.push('/'),
                        'b' => out.push('\x08'),
                        'f' => out.push('\x0c'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'u' => {
                            let mut hex = String::with_capacity(4);
                            for _ in 0..4 {
                                let hc = self.next_char().ok_or_else(|| {
                                    self.error("Incomplete \\uXXXX escape sequence")
                                })?;
                                hex.push(hc);
                            }
                            let code = u32::from_str_radix(&hex, 16)
                                .map_err(|_| self.error("Invalid hex in \\uXXXX escape"))?;
                            let decoded = char::from_u32(code)
                                .ok_or_else(|| self.error("Invalid Unicode scalar in \\uXXXX"))?;
                            out.push(decoded);
                        }
                        other => {
                            return Err(
                                self.error(&format!("Invalid escape character '\\{}'", other))
                            )
                        }
                    }
                }
                c if (c as u32) < 0x20 => {
                    return Err(self.error("Unescaped control character in JSON string"));
                }
                c => out.push(c),
            }
        }
        Err(self.error("Unterminated JSON string"))
    }

    fn parse_number(&mut self) -> Result<f64, String> {
        let start = self.pos;
        if self.peek() == Some('-') {
            self.pos += 1;
        }
        match self.peek() {
            Some('0') => {
                self.pos += 1;
            }
            Some('1'..='9') => {
                self.pos += 1;
                while let Some('0'..='9') = self.peek() {
                    self.pos += 1;
                }
            }
            _ => return Err(self.error("Invalid number literal")),
        }

        if self.peek() == Some('.') {
            self.pos += 1;
            let mut has_frac = false;
            while let Some('0'..='9') = self.peek() {
                self.pos += 1;
                has_frac = true;
            }
            if !has_frac {
                return Err(self.error("Expected digits after decimal point"));
            }
        }

        if let Some('e' | 'E') = self.peek() {
            self.pos += 1;
            if let Some('+' | '-') = self.peek() {
                self.pos += 1;
            }
            let mut has_exp = false;
            while let Some('0'..='9') = self.peek() {
                self.pos += 1;
                has_exp = true;
            }
            if !has_exp {
                return Err(self.error("Expected digits in exponent"));
            }
        }

        let s: String = self.chars[start..self.pos].iter().collect();
        s.parse::<f64>()
            .map_err(|e| self.error(&format!("Invalid number '{}': {}", s, e)))
    }

    fn parse_array(&mut self) -> Result<JsonValue, String> {
        self.next_char(); // '['
        self.skip_whitespace();
        let mut items = Vec::new();
        if self.peek() == Some(']') {
            self.next_char();
            return Ok(JsonValue::Array(items));
        }
        loop {
            let val = self.parse_value()?;
            items.push(val);
            self.skip_whitespace();
            match self.peek() {
                Some(',') => {
                    self.next_char();
                    self.skip_whitespace();
                }
                Some(']') => {
                    self.next_char();
                    return Ok(JsonValue::Array(items));
                }
                _ => return Err(self.error("Expected ',' or ']' in array")),
            }
        }
    }

    fn parse_object(&mut self) -> Result<JsonValue, String> {
        self.next_char(); // '{'
        self.skip_whitespace();
        let mut map = BTreeMap::new();
        if self.peek() == Some('}') {
            self.next_char();
            return Ok(JsonValue::Object(map));
        }
        loop {
            self.skip_whitespace();
            if self.peek() != Some('"') {
                return Err(self.error("Expected string key in JSON object"));
            }
            let key = self.read_json_string()?;
            self.skip_whitespace();
            if self.next_char() != Some(':') {
                return Err(self.error("Expected ':' after object key"));
            }
            let val = self.parse_value()?;
            map.insert(key, val);
            self.skip_whitespace();
            match self.peek() {
                Some(',') => {
                    self.next_char();
                }
                Some('}') => {
                    self.next_char();
                    return Ok(JsonValue::Object(map));
                }
                _ => return Err(self.error("Expected ',' or '}' in object")),
            }
        }
    }
}

/// Semantic version `major.minor.patch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SemVer {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl SemVer {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    pub fn parse(input: &str) -> Result<Self, String> {
        let trimmed = input.trim();
        // Strip optional pre-release/build metadata if present after patch
        let core = trimmed.split(&['-', '+'][..]).next().unwrap_or(trimmed);
        let parts: Vec<&str> = core.split('.').collect();
        if parts.len() != 3 {
            return Err(format!(
                "Invalid SemVer '{}': expected 'major.minor.patch'",
                input
            ));
        }
        let major = parts[0]
            .parse::<u32>()
            .map_err(|_| format!("Invalid major version in '{}'", input))?;
        let minor = parts[1]
            .parse::<u32>()
            .map_err(|_| format!("Invalid minor version in '{}'", input))?;
        let patch = parts[2]
            .parse::<u32>()
            .map_err(|_| format!("Invalid patch version in '{}'", input))?;
        Ok(Self {
            major,
            minor,
            patch,
        })
    }
}

impl fmt::Display for SemVer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// SemVer requirement operator and target version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionReq {
    Any,
    Caret(SemVer),
    Tilde(SemVer),
    Ge(SemVer),
    Le(SemVer),
    Gt(SemVer),
    Lt(SemVer),
    Exact(SemVer),
}

impl VersionReq {
    pub fn parse(input: &str) -> Result<Self, String> {
        let s = input.trim();
        if s.is_empty() || s == "*" {
            return Ok(VersionReq::Any);
        }
        if let Some(rest) = s.strip_prefix(">=") {
            return Ok(VersionReq::Ge(SemVer::parse(rest)?));
        }
        if let Some(rest) = s.strip_prefix("<=") {
            return Ok(VersionReq::Le(SemVer::parse(rest)?));
        }
        if let Some(rest) = s.strip_prefix('>') {
            return Ok(VersionReq::Gt(SemVer::parse(rest)?));
        }
        if let Some(rest) = s.strip_prefix('<') {
            return Ok(VersionReq::Lt(SemVer::parse(rest)?));
        }
        if let Some(rest) = s.strip_prefix('^') {
            return Ok(VersionReq::Caret(SemVer::parse(rest)?));
        }
        if let Some(rest) = s.strip_prefix('~') {
            return Ok(VersionReq::Tilde(SemVer::parse(rest)?));
        }
        if let Some(rest) = s.strip_prefix('=') {
            return Ok(VersionReq::Exact(SemVer::parse(rest)?));
        }
        Ok(VersionReq::Exact(SemVer::parse(s)?))
    }

    pub fn matches(&self, v: &SemVer) -> bool {
        match self {
            VersionReq::Any => true,
            VersionReq::Exact(target) => v == target,
            VersionReq::Ge(target) => v >= target,
            VersionReq::Le(target) => v <= target,
            VersionReq::Gt(target) => v > target,
            VersionReq::Lt(target) => v < target,
            VersionReq::Caret(target) => {
                if v < target {
                    return false;
                }
                if target.major > 0 {
                    v.major == target.major
                } else if target.minor > 0 {
                    v.major == 0 && v.minor == target.minor
                } else {
                    v.major == 0 && v.minor == 0 && v.patch == target.patch
                }
            }
            VersionReq::Tilde(target) => {
                v >= target && v.major == target.major && v.minor == target.minor
            }
        }
    }
}

impl fmt::Display for VersionReq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VersionReq::Any => write!(f, "*"),
            VersionReq::Caret(v) => write!(f, "^{}", v),
            VersionReq::Tilde(v) => write!(f, "~{}", v),
            VersionReq::Ge(v) => write!(f, ">={}", v),
            VersionReq::Le(v) => write!(f, "<={}", v),
            VersionReq::Gt(v) => write!(f, ">{}", v),
            VersionReq::Lt(v) => write!(f, "<{}", v),
            VersionReq::Exact(v) => write!(f, "={}", v),
        }
    }
}

/// Dependency or conflict declaration (`"mod_id"` or `"mod_id@^1.0.0"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModDep {
    pub id: String,
    pub req: VersionReq,
}

impl ModDep {
    pub fn parse(spec: &str) -> Result<Self, String> {
        let trimmed = spec.trim();
        let (id, req) = if let Some((id_part, ver_part)) = trimmed.split_once('@') {
            (id_part.trim(), VersionReq::parse(ver_part)?)
        } else {
            (trimmed, VersionReq::Any)
        };
        if !is_valid_mod_id(id) {
            return Err(format!(
                "Invalid dependency mod id '{}': must match ^[A-Za-z0-9_-]+$",
                id
            ));
        }
        Ok(Self {
            id: id.to_string(),
            req,
        })
    }

    pub fn from_json(val: &JsonValue) -> Result<Self, String> {
        match val {
            JsonValue::String(s) => Self::parse(s),
            JsonValue::Object(obj) => {
                let id = obj
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "Dependency object missing string field 'id'".to_string())?;
                if !is_valid_mod_id(id) {
                    return Err(format!(
                        "Invalid dependency mod id '{}': must match ^[A-Za-z0-9_-]+$",
                        id
                    ));
                }
                let req = if let Some(ver_val) = obj.get("version") {
                    let ver_str = ver_val
                        .as_str()
                        .ok_or_else(|| "Dependency 'version' must be a string".to_string())?;
                    VersionReq::parse(ver_str)?
                } else {
                    VersionReq::Any
                };
                Ok(Self {
                    id: id.to_string(),
                    req,
                })
            }
            _ => Err("Dependency entry must be a string or object".to_string()),
        }
    }

    pub fn matches(&self, version: &SemVer) -> bool {
        self.req.matches(version)
    }
}

impl fmt::Display for ModDep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.req == VersionReq::Any {
            write!(f, "{}", self.id)
        } else {
            write!(f, "{}@{}", self.id, self.req)
        }
    }
}

pub fn is_valid_mod_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Where a mod runs and who must have it (HOSTING_AND_MODS.md section 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ModKind {
    /// Runs on the server only; clients need nothing.
    ServerRules,
    /// Runs on server and client; the client must hold the same mod set as the server.
    Shared,
    /// Runs on the client only (skins, crosshairs, sound packs).
    ClientVisual,
}

pub const MOD_KINDS: &[&str] = &["server_rules", "shared", "client_visual"];

impl ModKind {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "server_rules" => Ok(ModKind::ServerRules),
            "shared" => Ok(ModKind::Shared),
            "client_visual" => Ok(ModKind::ClientVisual),
            other => Err(format!(
                "Unknown mod kind '{}': 'kind' must be one of {}",
                other,
                MOD_KINDS.join(", ")
            )),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ModKind::ServerRules => "server_rules",
            ModKind::Shared => "shared",
            ModKind::ClientVisual => "client_visual",
        }
    }

    /// Default for `requires_client`: only server-rule mods leave the client untouched.
    pub fn default_requires_client(&self) -> bool {
        !matches!(self, ModKind::ServerRules)
    }
}

impl fmt::Display for ModKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A `credited` mod names at least one work, and every credit is complete and under a license we allow (MOD-28).
// DC-DEFAULT: MOD-28 (proposed; USER DECISION 2026-11-03)
fn check_credits(policy: AssetPolicy, credits: &[ModCredit]) -> Result<(), String> {
    if policy == AssetPolicy::Credited && credits.is_empty() {
        return Err(
            "Manifest asset_policy 'credited' needs at least one entry in 'credits'".to_string(),
        );
    }
    for c in credits {
        for (name, v) in [
            ("title", &c.title),
            ("author", &c.author),
            ("license", &c.license),
            ("source", &c.source),
            ("changes", &c.changes),
        ] {
            if v.trim().is_empty() {
                return Err(format!(
                    "Manifest 'credits' entry '{}' has no '{name}'",
                    c.title
                ));
            }
        }
        if !CREDIT_LICENSES.contains(&c.license.as_str()) {
            return Err(format!(
                "Manifest 'credits' entry '{}': license '{}' is not one of {}",
                c.title,
                c.license,
                CREDIT_LICENSES.join(", ")
            ));
        }
    }
    Ok(())
}

/// Which assets a mod may reference or ship. Mods never contain Nexon assets
/// (HOSTING_AND_MODS.md section 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AssetPolicy {
    /// Only assets the mod authors made themselves, plus run-time procedural transforms of ROM data.
    #[default]
    OriginalOnly,
    /// Those, plus somebody else's openly licensed work with its credit in `credits` (MOD-28).
    Credited,
}

/// Licenses a `credited` mod may ship work under: they allow copying, changing and sharing with credit, without passing a license
/// on to the rest of the mod (MOD-28).
pub const CREDIT_LICENSES: &[&str] = &["CC-BY-4.0", "CC0-1.0"];

/// One work a mod ships that its authors did not make (MOD-28).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModCredit {
    pub title: String,
    pub author: String,
    pub license: String,
    pub source: String,
    pub changes: String,
}

pub const ASSET_POLICIES: &[&str] = &["original_only", "credited"];

impl AssetPolicy {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "original_only" => Ok(AssetPolicy::OriginalOnly),
            "credited" => Ok(AssetPolicy::Credited),
            other => Err(format!(
                "Unknown asset_policy '{}': 'asset_policy' must be one of {}",
                other,
                ASSET_POLICIES.join(", ")
            )),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            AssetPolicy::OriginalOnly => "original_only",
            AssetPolicy::Credited => "credited",
        }
    }
}

/// Hook namespaces a mod can declare as `hooks.<namespace>`; a hook name is `<namespace>.<name>`.
pub const HOOK_NAMESPACES: &[&str] = &["lobby", "combat", "render", "hud"];
/// Capabilities that are not content domains or hook namespaces.
// DC-DEFAULT: MOD-23 (proposed, project decision) `combat.no_recoil`: the set offers rooms without recoil
pub const PLAIN_CAPABILITIES: &[&str] = &[
    "events",
    "save",
    "assets",
    "world.full_visibility",
    "combat.no_recoil",
];
/// Hook namespaces that touch the client's view; only server-declared mods may use them.
pub const CLIENT_VIEW_CAPABILITIES: &[&str] = &["hooks.render", "hooks.hud"];
/// Content domains that stay legal for a client-only mod (presentation data).
const CLIENT_VISUAL_DOMAINS: &[&str] = &["hud", "music", "text"];

/// What a pack on a curated server entry may declare (LG-313). Everything else is refused: `hooks.lobby` and
/// `content.shop` could change rewards, EXP, GP, NX, prices or lobby rules, `world.full_visibility` and `hooks.render`
/// see through walls, `combat.no_recoil` and every other hook or plain capability are simply not on the list.
/// `content.maps` is on it (MAP-80): a community map pack is a curated pack (kind `shared`, `content.maps` + `assets`).
// DC-DEFAULT: LG-313 (proposed, project decision curated packs reviewed by the user alone; amended 2026-11-01
// for community maps on the Vanilla++ entry, MAP-80)
pub const CURATED_CAPABILITIES: &[&str] = &[
    "content.weapons",
    "content.text",
    "content.hud",
    "content.maps",
    "assets",
];

/// Why a pack may not load on a curated entry; `None` when it may. Kind `shared` only: a `client_visual` mod is no part
/// of the set hash and a `server_rules` mod would give the game server stats the client does not have. No permissions.
// DC-DEFAULT: LG-313 (proposed)
pub fn curated_refusal(m: &ModManifest) -> Option<String> {
    if m.kind != ModKind::Shared {
        return Some(format!(
            "kind '{}' is not allowed on a curated entry; a pack is kind 'shared'",
            m.kind
        ));
    }
    if let Some(p) = m.permissions.first() {
        return Some(format!(
            "permission '{p}' is not allowed on a curated entry"
        ));
    }
    m.capabilities
        .iter()
        .find(|c| !CURATED_CAPABILITIES.contains(&c.as_str()))
        .map(|c| {
            format!(
                "capability '{c}' is not allowed on a curated entry (allowed: {})",
                CURATED_CAPABILITIES.join(", ")
            )
        })
}

/// LG-314: why a pack's weapon content may not load on a curated entry: a new weapon row outside the reserved numbers.
// DC-DEFAULT: LG-314 (proposed)
pub fn curated_content_refusal(c: &dc_bute::modpatch::WeaponContent) -> Option<String> {
    use dc_bute::modpatch::{
        is_pack_item, is_variant_item, PatchOp, KEY_VARIANT_OF, PACK_ITEM_FIRST, PACK_ITEM_LAST,
        VARIANT_ITEM_FIRST, VARIANT_ITEM_LAST,
    };
    c.weapons
        .iter()
        .filter(|w| w.op == PatchOp::Register)
        .find_map(|w| {
            let n = i64::from(w.weapon);
            let variant = w.fields.contains_key(KEY_VARIANT_OF);
            // DC-DEFAULT: VAR-1 variants and new guns each keep to their own numbers
            if variant && !is_variant_item(n) {
                return Some(format!(
                    "variant row {n} is outside the variant numbers {VARIANT_ITEM_FIRST}..{VARIANT_ITEM_LAST}"
                ));
            }
            if !variant && (!is_pack_item(n) || is_variant_item(n)) {
                return Some(format!(
                    "new weapon row {n} is outside the pack numbers {PACK_ITEM_FIRST}..{PACK_ITEM_LAST}"
                ));
            }
            None
        })
}

/// Capability string for a content domain.
pub fn content_capability(domain: &str) -> String {
    format!("content.{}", domain)
}

/// Capability a hook name needs, or `None` when its namespace is not a known one.
pub fn hook_capability(hook_name: &str) -> Option<String> {
    let (ns, rest) = hook_name.split_once('.')?;
    if rest.is_empty() || !HOOK_NAMESPACES.contains(&ns) {
        return None;
    }
    Some(format!("hooks.{}", ns))
}

fn capability_help() -> String {
    format!(
        "content.<{}>, hooks.<{}>, {}",
        CONTENT_DOMAINS.join("|"),
        HOOK_NAMESPACES.join("|"),
        PLAIN_CAPABILITIES.join(", ")
    )
}

/// Checks one capability string against the allowed vocabulary.
pub fn validate_capability(cap: &str) -> Result<(), String> {
    let ok = if let Some(dom) = cap.strip_prefix("content.") {
        CONTENT_DOMAINS.contains(&dom)
    } else if let Some(ns) = cap.strip_prefix("hooks.") {
        HOOK_NAMESPACES.contains(&ns)
    } else {
        PLAIN_CAPABILITIES.contains(&cap)
    };
    if ok {
        Ok(())
    } else {
        Err(format!(
            "Unknown capability '{}': allowed capabilities are {}",
            cap,
            capability_help()
        ))
    }
}

/// Parsed and validated `manifest.json` structure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModManifest {
    pub id: String,
    pub name: String,
    pub version: SemVer,
    pub entry: String,
    pub api: u32,
    pub priority: i32,
    pub games: Vec<String>,
    pub dependencies: Vec<ModDep>,
    pub optional_dependencies: Vec<ModDep>,
    pub conflicts: Vec<ModDep>,
    pub permissions: Vec<String>,
    pub options_schema: Option<String>,
    pub github: Option<String>,
    /// Where the mod runs (required in `manifest.json`).
    pub kind: ModKind,
    /// Whether a client must hold this mod; defaults from `kind`.
    pub requires_client: bool,
    /// Content domains, hook namespaces and services the mod may use, sorted and unique.
    /// The host refuses any use beyond this list.
    pub capabilities: Vec<String>,
    pub asset_policy: AssetPolicy,
    /// What a `credited` mod ships that its authors did not make (MOD-28).
    pub credits: Vec<ModCredit>,
    /// What the mod does, shown in the server browser (required in `manifest.json`, 20..=500 characters).
    pub description: String,
    /// Who made it (required in `manifest.json`).
    pub author: String,
    /// `http(s)://` page of the mod, when given.
    pub homepage: Option<String>,
    /// Images for the server browser, `screenshots/<file>.png|jpg`, at most 6; at least one for kinds `shared`
    /// and `client_visual`. Their size and format are checked by `dc-modkit lint` (the files live in the package).
    pub screenshots: Vec<String>,
    /// Up to 8 short lowercase words for the browser's filter.
    pub tags: Vec<String>,
    pub changelog: Option<String>,
    /// MOD-27: the chat commands the mod's players can use, text for the detail panel; nothing runs from it.
    pub commands: Vec<ModCommand>,
    /// What the player can set on the Mods screen (MOD-24); kind `client_visual` only.
    pub settings: Vec<crate::settings::ModSetting>,
}

/// Browser metadata limits (manifest `description`, `screenshots`, `tags`, `author`).
pub const DESCRIPTION_CHARS: (usize, usize) = (20, 500);
pub const MAX_SCREENSHOTS: usize = 6;
pub const MAX_SCREENSHOT_BYTES: u64 = 1024 * 1024;
pub const MAX_TAGS: usize = 8;
pub const MAX_TAG_CHARS: usize = 20;
pub const MAX_AUTHOR_CHARS: usize = 100;

pub const MAX_COMMANDS: usize = 16;
pub const MAX_COMMAND_CHARS: usize = 24;
pub const MAX_COMMAND_TEXT_CHARS: usize = 120;

/// One line of a manifest's `commands` list: what to type and what it does (text only).
// DC-DEFAULT: MOD-27 (proposed, USER REQUEST 2026-11-01)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModCommand {
    pub command: String,
    pub description: String,
}

impl ModCommand {
    /// `/` and then 1 to 23 of `a-z 0-9 _ -`, and 1 to 120 characters of description on one line.
    pub fn is_valid(&self) -> bool {
        let n = self.command.chars().count();
        let d = self.description.trim().chars().count();
        self.command.starts_with('/')
            && (2..=MAX_COMMAND_CHARS).contains(&n)
            && self.command[1..]
                .bytes()
                .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
            && (1..=MAX_COMMAND_TEXT_CHARS).contains(&d)
            && !self.description.chars().any(char::is_control)
    }

    pub fn to_json(&self) -> JsonValue {
        let mut o = BTreeMap::new();
        o.insert(
            "command".to_string(),
            JsonValue::String(self.command.clone()),
        );
        o.insert(
            "description".to_string(),
            JsonValue::String(self.description.clone()),
        );
        JsonValue::Object(o)
    }
}

/// A screenshot path is `screenshots/<name>.png`, `.jpg` or `.jpeg` (any case), one level, plain characters.
pub fn is_valid_screenshot_path(p: &str) -> bool {
    let Some(name) = p.strip_prefix("screenshots/") else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    let ext_ok = [".png", ".jpg", ".jpeg"].iter().any(|e| lower.ends_with(e));
    let stem_len = name.rfind('.').unwrap_or(0);
    ext_ok
        && stem_len > 0
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        && !name.contains("..")
}

/// A tag is 1..=20 characters of `a-z`, `0-9` and `-`.
pub fn is_valid_tag(t: &str) -> bool {
    !t.is_empty()
        && t.len() <= MAX_TAG_CHARS
        && t.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

impl ModManifest {
    /// Manifest with defaults for everything but the identity fields (no capabilities).
    pub fn minimal(id: &str, name: &str, version: SemVer, kind: ModKind) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            version,
            entry: "main.lua".to_string(),
            api: 1,
            priority: 0,
            games: vec!["all".to_string()],
            dependencies: Vec::new(),
            optional_dependencies: Vec::new(),
            conflicts: Vec::new(),
            permissions: Vec::new(),
            options_schema: None,
            github: None,
            kind,
            requires_client: kind.default_requires_client(),
            capabilities: Vec::new(),
            asset_policy: AssetPolicy::OriginalOnly,
            credits: Vec::new(),
            description: String::new(),
            author: String::new(),
            homepage: None,
            screenshots: Vec::new(),
            tags: Vec::new(),
            changelog: None,
            commands: Vec::new(),
            settings: Vec::new(),
        }
    }

    /// The browser metadata rules (`description`, `author`, `homepage`, `screenshots`, `tags`). `from_json_value`
    /// applies them; `minimal` manifests (stand-ins built in code) do not pass them until filled in.
    pub fn check_metadata(&self) -> Result<(), String> {
        let n = self.description.trim().chars().count();
        let (lo, hi) = DESCRIPTION_CHARS;
        if n < lo || n > hi {
            return Err(format!(
                "Manifest 'description' is {} characters; it must be {} to {} (the server browser shows it)",
                n, lo, hi
            ));
        }
        let a = self.author.trim().chars().count();
        if a == 0 || a > MAX_AUTHOR_CHARS {
            return Err(format!(
                "Manifest 'author' must be 1 to {} characters",
                MAX_AUTHOR_CHARS
            ));
        }
        if let Some(h) = &self.homepage {
            let rest = h
                .strip_prefix("https://")
                .or_else(|| h.strip_prefix("http://"));
            if rest.is_none_or(|r| r.is_empty() || r.chars().any(char::is_whitespace)) {
                return Err(format!(
                    "Manifest 'homepage' '{}' is not an http:// or https:// URL",
                    h
                ));
            }
        }
        if self.screenshots.len() > MAX_SCREENSHOTS {
            return Err(format!(
                "Manifest 'screenshots' lists {} images; at most {} are allowed",
                self.screenshots.len(),
                MAX_SCREENSHOTS
            ));
        }
        for (i, p) in self.screenshots.iter().enumerate() {
            if !is_valid_screenshot_path(p) {
                return Err(format!(
                    "Invalid screenshot path '{}': must be 'screenshots/<name>.png' or '.jpg' (letters, digits, '_', '-')",
                    p
                ));
            }
            if self.screenshots[..i].contains(p) {
                return Err(format!(
                    "Manifest 'screenshots' lists '{}' more than once",
                    p
                ));
            }
        }
        if self.screenshots.is_empty() && self.kind != ModKind::ServerRules {
            return Err(format!(
                "Manifest 'screenshots' needs at least one image for kind '{}': players see it in the server browser",
                self.kind
            ));
        }
        if self.tags.len() > MAX_TAGS {
            return Err(format!(
                "Manifest 'tags' lists {} tags; at most {} are allowed",
                self.tags.len(),
                MAX_TAGS
            ));
        }
        for (i, t) in self.tags.iter().enumerate() {
            if !is_valid_tag(t) {
                return Err(format!(
                    "Invalid tag '{}': 1 to {} characters of a-z, 0-9 and '-'",
                    t, MAX_TAG_CHARS
                ));
            }
            if self.tags[..i].contains(t) {
                return Err(format!("Manifest 'tags' lists '{}' more than once", t));
            }
        }
        if self.commands.len() > MAX_COMMANDS {
            return Err(format!(
                "Manifest 'commands' lists {} commands; at most {} are allowed",
                self.commands.len(),
                MAX_COMMANDS
            ));
        }
        for (i, c) in self.commands.iter().enumerate() {
            if !c.is_valid() {
                return Err(format!(
                    "Invalid command '{}': 'command' is '/' and a-z, 0-9, '_', '-' (at most {} characters), 'description' is 1 to {} characters on one line",
                    c.command, MAX_COMMAND_CHARS, MAX_COMMAND_TEXT_CHARS
                ));
            }
            if self.commands[..i].iter().any(|x| x.command == c.command) {
                return Err(format!(
                    "Manifest 'commands' lists '{}' more than once",
                    c.command
                ));
            }
        }
        Ok(())
    }

    pub fn has_capability(&self, cap: &str) -> bool {
        self.capabilities.iter().any(|c| c == cap)
    }

    /// True when the manifest declares a render or HUD hook namespace.
    pub fn uses_client_view(&self) -> bool {
        CLIENT_VIEW_CAPABILITIES
            .iter()
            .any(|c| self.has_capability(c))
    }

    /// Cross-field rules between `kind`, `requires_client` and `capabilities`.
    pub fn check_consistency(&self) -> Result<(), String> {
        for cap in &self.capabilities {
            validate_capability(cap)?;
        }
        if self.requires_client != self.kind.default_requires_client() {
            return Err(if self.kind == ModKind::ServerRules {
                "Manifest 'requires_client' is true but kind 'server_rules' runs on the server only; \
                 use kind 'shared' for a mod the client must also load"
                    .to_string()
            } else {
                format!(
                    "Manifest 'requires_client' is false but kind '{}' runs on the client; \
                     use kind 'server_rules' for a mod only the server loads",
                    self.kind
                )
            });
        }
        // DC-DEFAULT: MOD-24 (proposed): a player's setting must not change what a shared or server mod does
        if !self.settings.is_empty() && self.kind != ModKind::ClientVisual {
            return Err(format!(
                "Manifest 'settings' is only for kind 'client_visual', not '{}'",
                self.kind
            ));
        }
        match self.kind {
            ModKind::ServerRules => {
                for cap in CLIENT_VIEW_CAPABILITIES {
                    if self.has_capability(cap) {
                        return Err(format!(
                            "Capability '{}' is not allowed for kind 'server_rules': render and HUD \
                             hooks run on the client; use kind 'shared'",
                            cap
                        ));
                    }
                }
            }
            ModKind::ClientVisual => {
                for cap in &self.capabilities {
                    // DC-DEFAULT: MOD-22 (proposed, project decision): turning the match's culling off is a server act
                    // DC-DEFAULT: MOD-23 (proposed, project decision): so is offering rooms without recoil
                    let server_only = matches!(
                        cap.as_str(),
                        "hooks.lobby"
                            | "hooks.combat"
                            | "world.full_visibility"
                            | "combat.no_recoil"
                    ) || cap
                        .strip_prefix("content.")
                        .is_some_and(|d| !CLIENT_VISUAL_DOMAINS.contains(&d));
                    if server_only {
                        return Err(format!(
                            "Capability '{}' is not allowed for kind 'client_visual': the server \
                             decides lobby, combat and gameplay content; use kind 'shared'",
                            cap
                        ));
                    }
                }
            }
            ModKind::Shared => {}
        }
        Ok(())
    }

    pub fn from_file(path: &Path) -> Result<Self, String> {
        let content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read manifest '{}': {}", path.display(), e))?;
        Self::from_json_str(&content)
    }

    pub fn from_json_str(json: &str) -> Result<Self, String> {
        let val = JsonValue::parse(json)?;
        Self::from_json_value(&val)
    }

    pub fn from_json_value(val: &JsonValue) -> Result<Self, String> {
        let obj = val
            .as_object()
            .ok_or_else(|| "Manifest root must be a JSON object".to_string())?;

        let id = obj
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Manifest missing required string field 'id'".to_string())?
            .to_string();
        if !is_valid_mod_id(&id) {
            return Err(format!(
                "Invalid mod id '{}': must be non-empty and match ^[A-Za-z0-9_-]+$",
                id
            ));
        }

        let name = obj
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Manifest missing required string field 'name'".to_string())?
            .to_string();
        if name.trim().is_empty() {
            return Err("Manifest 'name' cannot be empty".to_string());
        }

        let version_str = obj
            .get("version")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Manifest missing required string field 'version'".to_string())?;
        let version = SemVer::parse(version_str)?;

        let entry = match obj.get("entry") {
            Some(v) => {
                let s = v
                    .as_str()
                    .ok_or_else(|| "Manifest 'entry' must be a string".to_string())?;
                if s.is_empty()
                    || s.contains("..")
                    || s.starts_with('/')
                    || s.starts_with('\\')
                    || s.contains(':')
                {
                    return Err(format!("Invalid manifest entry path '{}'", s));
                }
                s.to_string()
            }
            None => "main.lua".to_string(),
        };

        let api = match obj.get("api") {
            Some(v) => v
                .as_u32()
                .ok_or_else(|| "Manifest 'api' must be an unsigned integer".to_string())?,
            None => 1,
        };

        let priority = match obj.get("priority") {
            Some(v) => v
                .as_i32()
                .ok_or_else(|| "Manifest 'priority' must be an integer".to_string())?,
            None => 0,
        };

        let games = match obj.get("games") {
            Some(v) => {
                let arr = v
                    .as_array()
                    .ok_or_else(|| "Manifest 'games' must be an array of strings".to_string())?;
                let mut list = Vec::with_capacity(arr.len());
                for item in arr {
                    let s = item
                        .as_str()
                        .ok_or_else(|| "Manifest 'games' elements must be strings".to_string())?;
                    list.push(s.to_string());
                }
                list
            }
            None => vec!["all".to_string()],
        };

        let dependencies = parse_dep_list(obj.get("dependencies"), "dependencies")?;
        let optional_dependencies =
            parse_dep_list(obj.get("optional_dependencies"), "optional_dependencies")?;

        let mut conflicts = parse_dep_list(obj.get("conflicts"), "conflicts")?;
        let incompatible = parse_dep_list(obj.get("incompatible"), "incompatible")?;
        for inc in incompatible {
            if !conflicts.contains(&inc) {
                conflicts.push(inc);
            }
        }

        let permissions = match obj.get("permissions") {
            Some(v) => {
                let arr = v.as_array().ok_or_else(|| {
                    "Manifest 'permissions' must be an array of strings".to_string()
                })?;
                let mut perms = Vec::with_capacity(arr.len());
                for item in arr {
                    let p = item.as_str().ok_or_else(|| {
                        "Manifest 'permissions' elements must be strings".to_string()
                    })?;
                    if !ALLOWED_PERMISSIONS.contains(&p) {
                        return Err(format!(
                            "Unknown permission '{}': allowed permissions are {:?}",
                            p, ALLOWED_PERMISSIONS
                        ));
                    }
                    perms.push(p.to_string());
                }
                perms
            }
            None => Vec::new(),
        };

        let options_schema = match obj.get("options_schema") {
            Some(JsonValue::Null) | None => None,
            Some(v) => Some(
                v.as_str()
                    .ok_or_else(|| "Manifest 'options_schema' must be a string".to_string())?
                    .to_string(),
            ),
        };

        let github = match obj.get("github") {
            Some(JsonValue::Null) | None => None,
            Some(v) => Some(
                v.as_str()
                    .ok_or_else(|| "Manifest 'github' must be a string".to_string())?
                    .to_string(),
            ),
        };

        let kind = match obj.get("kind") {
            None | Some(JsonValue::Null) => {
                return Err(format!(
                    "Manifest missing required field 'kind' (one of {})",
                    MOD_KINDS.join(", ")
                ))
            }
            Some(v) => ModKind::parse(
                v.as_str()
                    .ok_or_else(|| "Manifest 'kind' must be a string".to_string())?,
            )?,
        };

        let requires_client = match obj.get("requires_client") {
            None | Some(JsonValue::Null) => kind.default_requires_client(),
            Some(v) => v
                .as_bool()
                .ok_or_else(|| "Manifest 'requires_client' must be true or false".to_string())?,
        };

        let mut capabilities = match obj.get("capabilities") {
            None | Some(JsonValue::Null) => Vec::new(),
            Some(v) => {
                let arr = v.as_array().ok_or_else(|| {
                    "Manifest 'capabilities' must be an array of strings".to_string()
                })?;
                let mut caps: Vec<String> = Vec::with_capacity(arr.len());
                for item in arr {
                    let c = item.as_str().ok_or_else(|| {
                        "Manifest 'capabilities' elements must be strings".to_string()
                    })?;
                    validate_capability(c)?;
                    if caps.iter().any(|x| x == c) {
                        return Err(format!(
                            "Manifest 'capabilities' lists '{}' more than once",
                            c
                        ));
                    }
                    caps.push(c.to_string());
                }
                caps
            }
        };
        capabilities.sort();

        let asset_policy = match obj.get("asset_policy") {
            None | Some(JsonValue::Null) => AssetPolicy::default(),
            Some(v) => AssetPolicy::parse(
                v.as_str()
                    .ok_or_else(|| "Manifest 'asset_policy' must be a string".to_string())?,
            )?,
        };

        let mut credits = Vec::new();
        match obj.get("credits") {
            None | Some(JsonValue::Null) => {}
            Some(JsonValue::Array(rows)) => {
                for row in rows {
                    let JsonValue::Object(o) = row else {
                        return Err("Manifest 'credits' must hold objects".to_string());
                    };
                    let text =
                        |k: &str| o.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
                    credits.push(ModCredit {
                        title: text("title"),
                        author: text("author"),
                        license: text("license"),
                        source: text("source"),
                        changes: text("changes"),
                    });
                }
            }
            Some(_) => return Err("Manifest 'credits' must be an array".to_string()),
        }
        check_credits(asset_policy, &credits)?;

        let req_str = |key: &str, what: &str| -> Result<String, String> {
            match obj.get(key) {
                None | Some(JsonValue::Null) => Err(format!(
                    "Manifest missing required string field '{}' ({})",
                    key, what
                )),
                Some(v) => v
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("Manifest '{}' must be a string", key)),
            }
        };
        let opt_str = |key: &str| -> Result<Option<String>, String> {
            match obj.get(key) {
                None | Some(JsonValue::Null) => Ok(None),
                Some(v) => v
                    .as_str()
                    .map(|s| Some(s.to_string()))
                    .ok_or_else(|| format!("Manifest '{}' must be a string", key)),
            }
        };
        let str_list = |key: &str| -> Result<Vec<String>, String> {
            match obj.get(key) {
                None | Some(JsonValue::Null) => Ok(Vec::new()),
                Some(v) => {
                    let arr = v
                        .as_array()
                        .ok_or_else(|| format!("Manifest '{}' must be an array of strings", key))?;
                    arr.iter()
                        .map(|i| {
                            i.as_str().map(str::to_string).ok_or_else(|| {
                                format!("Manifest '{}' elements must be strings", key)
                            })
                        })
                        .collect()
                }
            }
        };
        let description = req_str(
            "description",
            "20 to 500 characters saying what the mod does, shown in the server browser",
        )?;
        let author = req_str("author", "who made the mod")?;
        let homepage = opt_str("homepage")?;
        let changelog = opt_str("changelog")?;
        let screenshots = str_list("screenshots")?;
        let tags = str_list("tags")?;
        let commands = match obj.get("commands") {
            None | Some(JsonValue::Null) => Vec::new(),
            Some(v) => v
                .as_array()
                .ok_or("Manifest 'commands' must be an array of {command, description}")?
                .iter()
                .map(|c| {
                    let field = |k: &str| c.get(k).and_then(|x| x.as_str()).map(str::to_string);
                    match (field("command"), field("description")) {
                        (Some(command), Some(description)) => Ok(ModCommand { command, description }),
                        _ => Err("Manifest 'commands' elements need the strings 'command' and 'description'".to_string()),
                    }
                })
                .collect::<Result<Vec<_>, String>>()?,
        };

        let manifest = Self {
            id,
            name,
            version,
            entry,
            api,
            priority,
            games,
            dependencies,
            optional_dependencies,
            conflicts,
            permissions,
            options_schema,
            github,
            kind,
            requires_client,
            capabilities,
            asset_policy,
            credits,
            description,
            author,
            homepage,
            screenshots,
            tags,
            changelog,
            commands,
            settings: crate::settings::parse_settings(obj.get("settings"))?,
        };
        manifest.check_consistency()?;
        manifest.check_metadata()?;
        Ok(manifest)
    }

    pub fn to_json_value(&self) -> JsonValue {
        let mut obj = BTreeMap::new();
        obj.insert("id".to_string(), JsonValue::String(self.id.clone()));
        obj.insert("name".to_string(), JsonValue::String(self.name.clone()));
        obj.insert(
            "version".to_string(),
            JsonValue::String(self.version.to_string()),
        );
        obj.insert("entry".to_string(), JsonValue::String(self.entry.clone()));
        obj.insert("api".to_string(), JsonValue::Number(self.api as f64));
        obj.insert(
            "priority".to_string(),
            JsonValue::Number(self.priority as f64),
        );
        obj.insert(
            "games".to_string(),
            JsonValue::Array(
                self.games
                    .iter()
                    .map(|g| JsonValue::String(g.clone()))
                    .collect(),
            ),
        );
        obj.insert(
            "dependencies".to_string(),
            JsonValue::Array(
                self.dependencies
                    .iter()
                    .map(|d| JsonValue::String(d.to_string()))
                    .collect(),
            ),
        );
        obj.insert(
            "optional_dependencies".to_string(),
            JsonValue::Array(
                self.optional_dependencies
                    .iter()
                    .map(|d| JsonValue::String(d.to_string()))
                    .collect(),
            ),
        );
        obj.insert(
            "conflicts".to_string(),
            JsonValue::Array(
                self.conflicts
                    .iter()
                    .map(|d| JsonValue::String(d.to_string()))
                    .collect(),
            ),
        );
        obj.insert(
            "permissions".to_string(),
            JsonValue::Array(
                self.permissions
                    .iter()
                    .map(|p| JsonValue::String(p.clone()))
                    .collect(),
            ),
        );
        obj.insert(
            "kind".to_string(),
            JsonValue::String(self.kind.as_str().to_string()),
        );
        obj.insert(
            "requires_client".to_string(),
            JsonValue::Bool(self.requires_client),
        );
        obj.insert(
            "capabilities".to_string(),
            JsonValue::Array(
                self.capabilities
                    .iter()
                    .map(|c| JsonValue::String(c.clone()))
                    .collect(),
            ),
        );
        obj.insert(
            "asset_policy".to_string(),
            JsonValue::String(self.asset_policy.as_str().to_string()),
        );
        if !self.credits.is_empty() {
            let row = |c: &ModCredit| {
                let mut o = std::collections::BTreeMap::new();
                for (k, v) in [
                    ("title", &c.title),
                    ("author", &c.author),
                    ("license", &c.license),
                    ("source", &c.source),
                    ("changes", &c.changes),
                ] {
                    o.insert(k.to_string(), JsonValue::String(v.clone()));
                }
                JsonValue::Object(o.into_iter().collect())
            };
            obj.insert(
                "credits".to_string(),
                JsonValue::Array(self.credits.iter().map(row).collect()),
            );
        }
        if let Some(schema) = &self.options_schema {
            obj.insert(
                "options_schema".to_string(),
                JsonValue::String(schema.clone()),
            );
        }
        if let Some(gh) = &self.github {
            obj.insert("github".to_string(), JsonValue::String(gh.clone()));
        }
        obj.insert(
            "description".to_string(),
            JsonValue::String(self.description.clone()),
        );
        obj.insert("author".to_string(), JsonValue::String(self.author.clone()));
        if let Some(h) = &self.homepage {
            obj.insert("homepage".to_string(), JsonValue::String(h.clone()));
        }
        let strings =
            |v: &[String]| JsonValue::Array(v.iter().cloned().map(JsonValue::String).collect());
        obj.insert("screenshots".to_string(), strings(&self.screenshots));
        if !self.tags.is_empty() {
            obj.insert("tags".to_string(), strings(&self.tags));
        }
        if let Some(c) = &self.changelog {
            obj.insert("changelog".to_string(), JsonValue::String(c.clone()));
        }
        if !self.commands.is_empty() {
            obj.insert(
                "commands".to_string(),
                JsonValue::Array(self.commands.iter().map(ModCommand::to_json).collect()),
            );
        }
        if !self.settings.is_empty() {
            obj.insert(
                "settings".to_string(),
                JsonValue::Array(self.settings.iter().map(|s| s.to_json()).collect()),
            );
        }
        JsonValue::Object(obj)
    }

    pub fn to_json_pretty(&self) -> String {
        self.to_json_value().to_json_pretty()
    }
}

fn parse_dep_list(val: Option<&JsonValue>, field_name: &str) -> Result<Vec<ModDep>, String> {
    let Some(v) = val else {
        return Ok(Vec::new());
    };
    let arr = v
        .as_array()
        .ok_or_else(|| format!("Manifest '{}' must be an array", field_name))?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        out.push(ModDep::from_json(item)?);
    }
    Ok(out)
}
