//! Declared mod settings (MOD-24): the manifest's `settings` list, the check of a stored value against its
//! declaration, and the values a mod reads through the read-only `mod.settings`.
//!
//! Provenance: ORIGINAL (DeCombat's own mod API; the 2009 client has no mods).
// DC-DEFAULT: MOD-24 (proposed, project decision) typed mod settings, client_visual only, see DC_DEFAULTS.md

use std::collections::{BTreeMap, BTreeSet};

use crate::hudkeys::parse_rgba_hex;
use crate::manifest::JsonValue;
use crate::value::LuaValue;

pub const MAX_SETTINGS: usize = 16;
pub const MAX_SETTING_KEY_CHARS: usize = 32;
pub const MAX_SETTING_LABEL_CHARS: usize = 40;
pub const MAX_SETTING_OPTIONS: usize = 32;
/// A number's grid at most (a share code holds its index in two bytes).
pub const MAX_NUMBER_STEPS: f64 = 60000.0;

/// One value of a `choice`: what is stored and read by the mod, and what the player sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingOption {
    pub value: String,
    pub label: String,
}

/// A `number` setting's grid: the values `min + n * step` up to `max`. Its fields are finite (the declaration check).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NumberRange {
    pub min: f64,
    pub max: f64,
    pub step: f64,
}

impl Eq for NumberRange {}

impl NumberRange {
    /// The largest grid index.
    pub fn top(&self) -> u32 {
        ((self.max - self.min) / self.step).round() as u32
    }

    /// The value of grid index `n`, in stored form.
    pub fn at(&self, n: u32) -> String {
        format_number(self.min + n.min(self.top()) as f64 * self.step)
    }

    /// The grid index of `v`, `None` when `v` is out of range or off the grid.
    pub fn index(&self, v: f64) -> Option<u32> {
        if !v.is_finite() {
            return None;
        }
        let n = ((v - self.min) / self.step).round();
        let on_grid = (self.min + n * self.step - v).abs() <= self.step * 1e-6;
        (on_grid && n >= 0.0 && n <= self.top() as f64).then_some(n as u32)
    }
}

/// A number's stored text: at most six decimals, no trailing zeros, never `-0`.
pub fn format_number(v: f64) -> String {
    let t = format!("{v:.6}");
    let t = t.trim_end_matches('0').trim_end_matches('.');
    if t == "-0" || t.is_empty() {
        "0".to_string()
    } else {
        t.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingKind {
    Colour,
    Choice(Vec<SettingOption>),
    Bool,
    Number(NumberRange),
}

/// One declared setting. `default` is in the stored form of [`ModSetting::accept`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModSetting {
    pub key: String,
    pub label: String,
    pub kind: SettingKind,
    pub default: String,
    /// The heading the settings page groups this row under (absent: it stays under the heading before it).
    pub group: Option<String>,
}

fn plain_word(s: &str) -> bool {
    !s.is_empty()
        && s.chars().count() <= MAX_SETTING_KEY_CHARS
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

impl ModSetting {
    /// `text` in stored form (colour `#rrggbb` / `#rrggbbaa` in lower case, a choice's value, `true` / `false`, a
    /// number on its grid in [`format_number`]'s form);
    /// `None` when the declaration does not allow it.
    pub fn accept(&self, text: &str) -> Option<String> {
        let t = text.trim();
        match &self.kind {
            SettingKind::Colour => parse_rgba_hex(t)
                .map(|_| format!("#{}", t.trim_start_matches('#').to_ascii_lowercase())),
            SettingKind::Choice(o) => o.iter().any(|x| x.value == t).then(|| t.to_string()),
            SettingKind::Bool => matches!(t, "true" | "false").then(|| t.to_string()),
            SettingKind::Number(r) => {
                let n = r.index(t.parse::<f64>().ok()?)?;
                Some(r.at(n))
            }
        }
    }

    /// What the mod reads for the stored form `text`.
    pub fn lua_value(&self, text: &str) -> LuaValue {
        match self.kind {
            SettingKind::Bool => LuaValue::Bool(text == "true"),
            SettingKind::Number(_) => LuaValue::Number(text.parse().unwrap_or(0.0)),
            _ => LuaValue::String(text.to_string()),
        }
    }

    fn from_json(i: usize, v: &JsonValue) -> Result<ModSetting, String> {
        let e = |m: String| format!("Manifest 'settings[{i}]': {m}");
        let o = v.as_object().ok_or_else(|| e("must be an object".into()))?;
        let field = |k: &str| {
            o.get(k)
                .and_then(|x| x.as_str())
                .map(str::to_string)
                .ok_or_else(|| e(format!("missing string field '{k}'")))
        };
        let key = field("key")?;
        if !plain_word(&key) {
            return Err(e(format!(
                "key '{key}' must be 1 to {MAX_SETTING_KEY_CHARS} of a-z, 0-9, _"
            )));
        }
        let label = field("label")?;
        let n = label.trim().chars().count();
        if n == 0 || n > MAX_SETTING_LABEL_CHARS {
            return Err(e(format!(
                "label must be 1 to {MAX_SETTING_LABEL_CHARS} characters"
            )));
        }
        let kind = match field("type")?.as_str() {
            "colour" => SettingKind::Colour,
            "bool" => SettingKind::Bool,
            "number" => {
                let num = |k: &str| match o.get(k) {
                    Some(JsonValue::Number(v)) if v.is_finite() => Ok(*v),
                    _ => Err(e(format!("a number needs a numeric '{k}'"))),
                };
                let r = NumberRange {
                    min: num("min")?,
                    max: num("max")?,
                    step: num("step")?,
                };
                if r.step <= 0.0 || r.min >= r.max {
                    return Err(e("a number needs min < max and step > 0".into()));
                }
                let steps = (r.max - r.min) / r.step;
                if steps > MAX_NUMBER_STEPS || (steps - steps.round()).abs() > 1e-6 {
                    return Err(e(format!(
                        "max must be min plus a whole number of steps, at most {MAX_NUMBER_STEPS}"
                    )));
                }
                SettingKind::Number(r)
            }
            "choice" => {
                let list = o
                    .get("options")
                    .and_then(|x| x.as_array())
                    .ok_or_else(|| e("a choice needs an 'options' array".into()))?;
                if list.len() < 2 || list.len() > MAX_SETTING_OPTIONS {
                    return Err(e(format!(
                        "a choice needs 2 to {MAX_SETTING_OPTIONS} options"
                    )));
                }
                let mut seen = BTreeSet::new();
                let mut options = Vec::new();
                for x in list {
                    // "value", or {"value": ..., "label": ...}
                    let (value, shown) = match (x.as_str(), x.as_object()) {
                        (Some(s), _) => (s.to_string(), s.to_string()),
                        (_, Some(m)) => {
                            let val = m.get("value").and_then(|y| y.as_str());
                            let val = val.ok_or_else(|| e("an option needs a 'value'".into()))?;
                            let shown = m.get("label").and_then(|y| y.as_str()).unwrap_or(val);
                            (val.to_string(), shown.to_string())
                        }
                        _ => return Err(e("an option is a string or {value, label}".into())),
                    };
                    if !plain_word(&value) {
                        return Err(e(format!("option '{value}' must be a-z, 0-9, _")));
                    }
                    let n = shown.trim().chars().count();
                    if n == 0 || n > MAX_SETTING_LABEL_CHARS {
                        return Err(e(format!(
                            "the label of option '{value}' is empty or too long"
                        )));
                    }
                    if !seen.insert(value.clone()) {
                        return Err(e(format!("option '{value}' appears twice")));
                    }
                    options.push(SettingOption {
                        value,
                        label: shown,
                    });
                }
                SettingKind::Choice(options)
            }
            other => {
                return Err(e(format!(
                    "unknown type '{other}'; the types are: colour, choice, bool, number"
                )))
            }
        };
        let group = match o.get("group") {
            None => None,
            Some(g) => {
                let g = g.as_str().map(str::trim).unwrap_or("");
                if g.is_empty() || g.chars().count() > MAX_SETTING_LABEL_CHARS {
                    return Err(e(format!(
                        "group must be 1 to {MAX_SETTING_LABEL_CHARS} characters"
                    )));
                }
                Some(g.to_string())
            }
        };
        let mut s = ModSetting {
            key,
            label,
            kind,
            default: String::new(),
            group,
        };
        let given = match (o.get("default"), &s.kind) {
            (Some(JsonValue::Bool(b)), SettingKind::Bool) => b.to_string(),
            (Some(JsonValue::Number(n)), SettingKind::Number(_)) => format_number(*n),
            (Some(JsonValue::String(t)), SettingKind::Colour | SettingKind::Choice(_)) => t.clone(),
            (None, _) => return Err(e("missing 'default'".into())),
            _ => return Err(e("'default' has the wrong type".into())),
        };
        s.default = s
            .accept(&given)
            .ok_or_else(|| e(format!("default '{given}' is not a value of this setting")))?;
        Ok(s)
    }

    pub fn to_json(&self) -> JsonValue {
        let mut o = BTreeMap::new();
        let s = |t: &str| JsonValue::String(t.to_string());
        o.insert("key".to_string(), s(&self.key));
        o.insert("label".to_string(), s(&self.label));
        let ty = match &self.kind {
            SettingKind::Colour => "colour",
            SettingKind::Bool => "bool",
            SettingKind::Number(r) => {
                o.insert("min".to_string(), JsonValue::Number(r.min));
                o.insert("max".to_string(), JsonValue::Number(r.max));
                o.insert("step".to_string(), JsonValue::Number(r.step));
                "number"
            }
            SettingKind::Choice(list) => {
                let opts = list
                    .iter()
                    .map(|x| {
                        let mut m = BTreeMap::new();
                        m.insert("value".to_string(), s(&x.value));
                        m.insert("label".to_string(), s(&x.label));
                        JsonValue::Object(m)
                    })
                    .collect();
                o.insert("options".to_string(), JsonValue::Array(opts));
                "choice"
            }
        };
        o.insert("type".to_string(), s(ty));
        if let Some(g) = &self.group {
            o.insert("group".to_string(), s(g));
        }
        o.insert(
            "default".to_string(),
            match self.kind {
                SettingKind::Bool => JsonValue::Bool(self.default == "true"),
                SettingKind::Number(_) => JsonValue::Number(self.default.parse().unwrap_or(0.0)),
                _ => s(&self.default),
            },
        );
        JsonValue::Object(o)
    }
}

/// The manifest's `settings` field (absent: none).
pub fn parse_settings(val: Option<&JsonValue>) -> Result<Vec<ModSetting>, String> {
    let Some(v) = val else {
        return Ok(Vec::new());
    };
    let list = v.as_array().ok_or("Manifest 'settings' must be an array")?;
    if list.len() > MAX_SETTINGS {
        return Err(format!(
            "Manifest 'settings' lists more than {MAX_SETTINGS} settings"
        ));
    }
    let mut out: Vec<ModSetting> = Vec::new();
    for (i, x) in list.iter().enumerate() {
        let s = ModSetting::from_json(i, x)?;
        if out.iter().any(|o| o.key == s.key) {
            return Err(format!(
                "Manifest 'settings': key '{}' appears twice",
                s.key
            ));
        }
        out.push(s);
    }
    Ok(out)
}

/// Every declared key with its stored value, or the default where the stored one is absent or not allowed. Keys the
/// declaration does not name are dropped.
pub fn resolve_settings(
    declared: &[ModSetting],
    stored: Option<&BTreeMap<String, String>>,
) -> BTreeMap<String, String> {
    declared
        .iter()
        .map(|d| {
            let v = stored
                .and_then(|m| m.get(&d.key))
                .and_then(|t| d.accept(t))
                .unwrap_or_else(|| d.default.clone());
            (d.key.clone(), v)
        })
        .collect()
}

// ------------------------------------------------------------------ share codes
// DC-DEFAULT: MOD-26 (proposed, project decision) our own share code of a mod's settings, see DC_DEFAULTS.md

/// Every code starts with this.
pub const SHARE_PREFIX: &str = "DCS";
const SHARE_VERSION: u8 = 1;
const SHARE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

fn fnv16(bytes: &[u8]) -> u16 {
    let mut h: u32 = 0x811c_9dc5;
    for &b in bytes {
        h = (h ^ b as u32).wrapping_mul(0x0100_0193);
    }
    (h ^ (h >> 16)) as u16
}

/// What a code is valid for: the mod and the shape of its declaration.
fn share_layout(id: &str, decl: &[ModSetting]) -> u16 {
    let mut text = id.to_string();
    for d in decl {
        text.push('|');
        text.push_str(&d.key);
        match &d.kind {
            SettingKind::Colour => text.push_str(":c"),
            SettingKind::Bool => text.push_str(":b"),
            SettingKind::Choice(o) => {
                for x in o {
                    text.push(':');
                    text.push_str(&x.value);
                }
            }
            SettingKind::Number(r) => text.push_str(&format!(":{}:{}:{}", r.min, r.max, r.step)),
        }
    }
    fnv16(text.as_bytes())
}

/// All of `values` as a code the player can copy: `DCS-` and base-32 groups (no I, L, O, U). A missing or bad value
/// counts as its default.
pub fn encode_share_code(
    id: &str,
    decl: &[ModSetting],
    values: &BTreeMap<String, String>,
) -> String {
    let mut b = vec![SHARE_VERSION];
    b.extend_from_slice(&share_layout(id, decl).to_be_bytes());
    for d in decl {
        let v = values
            .get(&d.key)
            .and_then(|t| d.accept(t))
            .unwrap_or_else(|| d.default.clone());
        match &d.kind {
            SettingKind::Colour => b.extend_from_slice(&parse_rgba_hex(&v).unwrap_or([255; 4])),
            SettingKind::Bool => b.push((v == "true") as u8),
            SettingKind::Choice(o) => {
                b.push(o.iter().position(|x| x.value == v).unwrap_or(0) as u8)
            }
            SettingKind::Number(r) => {
                let n = v.parse().ok().and_then(|x| r.index(x)).unwrap_or(0);
                b.extend_from_slice(&(n as u16).to_be_bytes());
            }
        }
    }
    let sum = fnv16(&b);
    b.extend_from_slice(&sum.to_be_bytes());
    let mut out = String::from(SHARE_PREFIX);
    let (mut acc, mut bits, mut n) = (0u32, 0u32, 0usize);
    let mut push = |c: u32| {
        if n % 5 == 0 {
            out.push('-');
        }
        out.push(SHARE_ALPHABET[c as usize] as char);
        n += 1;
    };
    for x in b {
        acc = (acc << 8) | x as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            push((acc >> bits) & 31);
        }
        acc &= (1 << bits) - 1;
    }
    if bits > 0 {
        push((acc << (5 - bits)) & 31);
    }
    out
}

/// The values of a code, or the reason it is refused. Case, spaces and dashes do not matter.
pub fn decode_share_code(
    id: &str,
    decl: &[ModSetting],
    code: &str,
) -> Result<BTreeMap<String, String>, String> {
    const NOT_A_CODE: &str = "That is not a valid settings code.";
    let clean: String = code
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    let body = clean.strip_prefix(SHARE_PREFIX).ok_or(NOT_A_CODE)?;
    if body.len() > 256 {
        return Err(NOT_A_CODE.into());
    }
    let mut b = Vec::with_capacity(body.len() * 5 / 8);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in body.bytes() {
        let v = SHARE_ALPHABET
            .iter()
            .position(|x| *x == c)
            .ok_or(NOT_A_CODE)? as u32;
        acc = (acc << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            b.push((acc >> bits) as u8);
        }
        acc &= (1 << bits) - 1;
    }
    if acc != 0 || bits >= 5 || b.len() < 5 {
        return Err(NOT_A_CODE.into());
    }
    let (data, sum) = b.split_at(b.len() - 2);
    if fnv16(data).to_be_bytes() != sum || data[0] != SHARE_VERSION {
        return Err(NOT_A_CODE.into());
    }
    if data[1..3] != share_layout(id, decl).to_be_bytes() {
        return Err("That code belongs to another mod or another version of this one.".into());
    }
    let mut p = &data[3..];
    let mut take = |n: usize| -> Result<Vec<u8>, String> {
        if p.len() < n {
            return Err(NOT_A_CODE.to_string());
        }
        let (a, rest) = p.split_at(n);
        p = rest;
        Ok(a.to_vec())
    };
    let mut out = BTreeMap::new();
    for d in decl {
        let v = match &d.kind {
            SettingKind::Colour => {
                let c = take(4)?;
                colour_text([c[0], c[1], c[2], c[3]])
            }
            SettingKind::Bool => match take(1)?[0] {
                0 => "false".to_string(),
                1 => "true".to_string(),
                _ => return Err(NOT_A_CODE.into()),
            },
            SettingKind::Choice(o) => o.get(take(1)?[0] as usize).ok_or(NOT_A_CODE)?.value.clone(),
            SettingKind::Number(r) => {
                let c = take(2)?;
                let n = u16::from_be_bytes([c[0], c[1]]) as u32;
                if n > r.top() {
                    return Err(NOT_A_CODE.into());
                }
                r.at(n)
            }
        };
        out.insert(d.key.clone(), v);
    }
    if take(1).is_ok() {
        return Err(NOT_A_CODE.into());
    }
    Ok(out)
}

/// A colour in stored form: `#rrggbb`, with `aa` only when it is not opaque.
pub fn colour_text(c: [u8; 4]) -> String {
    if c[3] == 255 {
        format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
    } else {
        format!("#{:02x}{:02x}{:02x}{:02x}", c[0], c[1], c[2], c[3])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Result<Vec<ModSetting>, String> {
        parse_settings(Some(&JsonValue::parse(json).unwrap()))
    }

    const GOOD: &str = r##"[
        {"key": "colour", "label": "Colour", "type": "colour", "default": "#4080FF"},
        {"key": "style", "label": "Style", "type": "choice", "default": "b",
         "options": ["a", {"value": "b", "label": "Bee"}]},
        {"key": "kick", "label": "Kick", "type": "bool", "default": true}]"##;

    #[test]
    fn a_good_list_parses_and_round_trips() {
        let s = parse(GOOD).unwrap();
        assert_eq!(s[0].default, "#4080ff");
        assert_eq!(s[2].default, "true");
        let again = parse_settings(Some(&JsonValue::Array(
            s.iter().map(|x| x.to_json()).collect(),
        )));
        assert_eq!(again.unwrap(), s);
    }

    #[test]
    fn bad_declarations_are_refused() {
        for (bad, why) in [
            (
                r#"[{"key":"A","label":"x","type":"bool","default":true}]"#,
                "key",
            ),
            (
                r#"[{"key":"a","label":"","type":"bool","default":true}]"#,
                "label",
            ),
            (
                r#"[{"key":"a","label":"x","type":"slider","default":"1"}]"#,
                "unknown type",
            ),
            (
                r#"[{"key":"a","label":"x","type":"bool","default":"true"}]"#,
                "wrong type",
            ),
            (
                r#"[{"key":"a","label":"x","type":"colour","default":"blue"}]"#,
                "default",
            ),
            (
                r#"[{"key":"a","label":"x","type":"choice","default":"a","options":["a"]}]"#,
                "2 to",
            ),
            (
                r#"[{"key":"a","label":"x","type":"choice","default":"z","options":["a","b"]}]"#,
                "default",
            ),
            (
                r#"[{"key":"a","label":"x","type":"choice","default":"a","options":["a","a"]}]"#,
                "twice",
            ),
            (
                r#"[{"key":"a","label":"x","type":"bool","default":true},{"key":"a","label":"y","type":"bool","default":true}]"#,
                "twice",
            ),
            (
                r#"[{"key":"a","label":"x","type":"number","default":1,"max":2,"step":1}]"#,
                "'min'",
            ),
            (
                r#"[{"key":"a","label":"x","type":"number","default":1,"min":2,"max":1,"step":1}]"#,
                "min < max",
            ),
            (
                r#"[{"key":"a","label":"x","type":"number","default":1,"min":0,"max":2,"step":0}]"#,
                "step > 0",
            ),
            (
                r#"[{"key":"a","label":"x","type":"number","default":1,"min":0,"max":2,"step":0.7}]"#,
                "whole number",
            ),
            (
                r#"[{"key":"a","label":"x","type":"number","default":1,"min":0,"max":100000,"step":1}]"#,
                "whole number",
            ),
            (
                r#"[{"key":"a","label":"x","type":"number","default":3,"min":0,"max":2,"step":1}]"#,
                "default",
            ),
            (
                r#"[{"key":"a","label":"x","type":"number","default":0.3,"min":0,"max":2,"step":0.5}]"#,
                "default",
            ),
            (
                r#"[{"key":"a","label":"x","type":"number","default":"1","min":0,"max":2,"step":1}]"#,
                "wrong type",
            ),
            (
                r#"[{"key":"a","label":"x","type":"bool","default":true,"group":""}]"#,
                "group",
            ),
        ] {
            let e = parse(bad).unwrap_err();
            assert!(e.contains(why), "{bad}: {e}");
        }
    }

    #[test]
    fn a_bad_stored_value_falls_back_to_the_default() {
        let d = parse(GOOD).unwrap();
        let mut st = BTreeMap::new();
        st.insert("colour".to_string(), "not a colour".to_string());
        st.insert("style".to_string(), "a".to_string());
        st.insert("kick".to_string(), "1".to_string());
        st.insert("stale".to_string(), "x".to_string());
        let r = resolve_settings(&d, Some(&st));
        assert_eq!(r["colour"], "#4080ff");
        assert_eq!(r["style"], "a");
        assert_eq!(r["kick"], "true");
        assert!(!r.contains_key("stale"));
    }

    const NUMBERS: &str = r##"[
        {"key": "len", "label": "Length", "type": "number", "min": 0.5, "max": 40, "step": 0.5, "default": 7, "group": "Shape"},
        {"key": "gap", "label": "Gap", "type": "number", "min": -20, "max": 40, "step": 0.5, "default": 0},
        {"key": "thick", "label": "Thick", "type": "number", "min": 0.5, "max": 12, "step": 0.1, "default": 1.2},
        {"key": "colour", "label": "Colour", "type": "colour", "default": "#4080FF"},
        {"key": "style", "label": "Style", "type": "choice", "default": "b", "options": ["a", "b", "c"]},
        {"key": "kick", "label": "Kick", "type": "bool", "default": true}]"##;

    #[test]
    fn a_number_checks_its_grid_and_round_trips() {
        let d = parse(NUMBERS).unwrap();
        assert_eq!(d[0].group.as_deref(), Some("Shape"));
        assert_eq!(d[2].default, "1.2");
        assert_eq!(d[0].accept("6.5").as_deref(), Some("6.5"));
        assert_eq!(d[0].accept(" 7.0 ").as_deref(), Some("7"));
        assert_eq!(d[1].accept("-3.5").as_deref(), Some("-3.5"));
        assert_eq!(d[1].accept("0").as_deref(), Some("0"));
        assert_eq!(
            d[2].accept("1.7").as_deref(),
            Some("1.7"),
            "a decimal step lands on its grid"
        );
        for bad in ["6.3", "0", "40.5", "NaN", "inf", "", "seven"] {
            assert_eq!(d[0].accept(bad), None, "{bad}");
        }
        assert_eq!(d[0].lua_value("6.5"), LuaValue::Number(6.5));
        let again = parse_settings(Some(&JsonValue::Array(
            d.iter().map(|x| x.to_json()).collect(),
        )));
        assert_eq!(again.unwrap(), d);
        // a stored value out of range or off the grid reads as the default
        let st = [
            ("len".to_string(), "99".to_string()),
            ("gap".to_string(), "-3".to_string()),
        ];
        let r = resolve_settings(&d, Some(&st.into_iter().collect()));
        assert_eq!((r["len"].as_str(), r["gap"].as_str()), ("7", "-3"));
    }

    #[test]
    fn a_share_code_round_trips_and_a_bad_one_is_refused() {
        let d = parse(NUMBERS).unwrap();
        let st = [
            ("len".to_string(), "12.5".to_string()),
            ("gap".to_string(), "-3.5".to_string()),
            ("thick".to_string(), "2.3".to_string()),
            ("colour".to_string(), "#10203040".to_string()),
            ("style".to_string(), "c".to_string()),
            ("kick".to_string(), "false".to_string()),
        ];
        let values = resolve_settings(&d, Some(&st.into_iter().collect()));
        let code = encode_share_code("m", &d, &values);
        assert!(code.starts_with("DCS-") && code.len() < 40, "{code}");
        assert_eq!(decode_share_code("m", &d, &code).unwrap(), values);
        assert_eq!(
            decode_share_code("m", &d, &format!(" {} ", code.to_ascii_lowercase())).unwrap(),
            values
        );
        let defaults = resolve_settings(&d, None);
        assert_eq!(
            decode_share_code("m", &d, &encode_share_code("m", &d, &defaults)).unwrap(),
            defaults
        );
        // every changed character is caught
        for i in 4..code.len() {
            if code.as_bytes()[i] == b'-' {
                continue;
            }
            let mut bad = code.clone().into_bytes();
            bad[i] = if bad[i] == b'A' { b'B' } else { b'A' };
            let bad = String::from_utf8(bad).unwrap();
            assert!(decode_share_code("m", &d, &bad).is_err(), "{bad}");
        }
        for bad in [
            "",
            "hello",
            "DCS",
            "DCS-",
            "DCS-!!!!!",
            &code[..code.len() - 3],
            &format!("{code}00"),
        ] {
            assert!(decode_share_code("m", &d, bad).is_err(), "{bad}");
        }
        let e = decode_share_code("other", &d, &code).unwrap_err();
        assert!(e.contains("another mod"), "{e}");
        // an index past a number's grid, with a good checksum, is refused
        let short = parse(
            r#"[{"key":"n","label":"N","type":"number","min":0,"max":1,"step":1,"default":0}]"#,
        )
        .unwrap();
        let wide = parse(
            r#"[{"key":"n","label":"N","type":"number","min":0,"max":9,"step":1,"default":0}]"#,
        )
        .unwrap();
        let v = [("n".to_string(), "9".to_string())].into_iter().collect();
        assert!(decode_share_code("m", &short, &encode_share_code("m", &wide, &v)).is_err());
    }
}
