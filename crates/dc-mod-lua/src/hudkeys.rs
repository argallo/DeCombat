//! The key vocabulary of `mod.content.hud:override(key, value)` (CLI-1613) and the value check the host runs at load,
//! so an unknown key or a value of the wrong shape refuses the mod before any frame is drawn.
//!
//! Provenance: ORIGINAL (DeCombat's own mod API; the 2009 client has no HUD overrides).
// DC-DEFAULT: CLI-1613 (proposed) the HUD override keys, see DC_DEFAULTS.md

use crate::value::LuaValue;

/// The keys a mod may override, in the order the guide lists them.
pub const HUD_OVERRIDE_KEYS: &[&str] = &[
    "crosshair",
    "crosshair_colour",
    "hitmarker",
    "hud_scale",
    "crosshair_style",
    "crosshair_recoil",
    "crosshair_length",
    "crosshair_thickness",
    "crosshair_gap",
    "crosshair_dot",
    "crosshair_outline",
    "crosshair_t_style",
    "crosshair_recoil_scale",
];

/// The text values of `crosshair_recoil` (it also takes `true`, like `dynamic`, and `false`, like `static`).
pub const CROSSHAIR_RECOIL_MODES: &[&str] = &["dynamic", "static", "fire"];

/// The ranges of the drawn crosshair's numbers, in virtual pixels (both ends allowed); the factor has no unit.
pub const CROSSHAIR_LENGTH_RANGE: (f64, f64) = (0.0, 100.0);
pub const CROSSHAIR_THICKNESS_RANGE: (f64, f64) = (0.1, 30.0);
pub const CROSSHAIR_GAP_RANGE: (f64, f64) = (-50.0, 100.0);
pub const CROSSHAIR_OUTLINE_RANGE: (f64, f64) = (0.0, 10.0);
pub const CROSSHAIR_RECOIL_SCALE_RANGE: (f64, f64) = (0.0, 5.0);

/// The values of `crosshair_style` (CLI-1633).
pub const CROSSHAIR_STYLES: &[&str] = &["cross", "circle", "dot", "cross_dot", "circle_dot"];

/// The range `hud_scale` must lie in (both ends allowed).
pub const HUD_SCALE_RANGE: (f64, f64) = (0.5, 2.0);

/// `RRGGBB` or `RRGGBBAA`, with or without a leading `#`, as RGBA bytes (alpha 255 when absent).
pub fn parse_rgba_hex(text: &str) -> Option<[u8; 4]> {
    let h = text.trim().strip_prefix('#').unwrap_or(text.trim());
    if !(h.len() == 6 || h.len() == 8) || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([
        byte(0)?,
        byte(2)?,
        byte(4)?,
        if h.len() == 8 { byte(6)? } else { 255 },
    ])
}

/// Checks one override: the key must be in [`HUD_OVERRIDE_KEYS`] and the value of its shape. The message names the
/// vocabulary, as the manifest's "Unknown capability" message does.
pub fn check_hud_override(key: &str, value: &LuaValue) -> Result<(), String> {
    let texture = |v: &LuaValue| {
        match v {
        LuaValue::String(s) if !s.trim().is_empty() => Ok(()),
        other => Err(format!(
            "HUD override '{key}' must be a texture path string (a ROM path or mod.assets:path(...)), got {}",
            other.type_name()
        )),
    }
    };
    let ranged = |(lo, hi): (f64, f64)| match value.as_f64() {
        Some(n) if n.is_finite() && n >= lo && n <= hi => Ok(()),
        _ => Err(format!("HUD override '{key}' must be a number from {lo} to {hi}, got {value:?}")),
    };
    match key {
        "crosshair" | "hitmarker" => texture(value),
        "crosshair_colour" => match value {
            LuaValue::String(s) if parse_rgba_hex(s).is_some() => Ok(()),
            other => Err(format!(
                "HUD override 'crosshair_colour' must be an RGBA hex string such as \"00ff00\" or \"#00ff00c0\", got {}",
                match other {
                    LuaValue::String(s) => format!("{s:?}"),
                    o => o.type_name().to_string(),
                }
            )),
        },
        "hud_scale" => match value.as_f64() {
            Some(n) if n.is_finite() && n >= HUD_SCALE_RANGE.0 && n <= HUD_SCALE_RANGE.1 => Ok(()),
            _ => Err(format!(
                "HUD override 'hud_scale' must be a number from {} to {}, got {:?}",
                HUD_SCALE_RANGE.0, HUD_SCALE_RANGE.1, value
            )),
        },
        "crosshair_style" => match value {
            LuaValue::String(s) if CROSSHAIR_STYLES.contains(&s.as_str()) => Ok(()),
            other => Err(format!(
                "HUD override 'crosshair_style' must be one of {}, got {:?}",
                CROSSHAIR_STYLES.join(", "),
                other
            )),
        },
        "crosshair_recoil" => match value {
            LuaValue::Bool(_) => Ok(()),
            LuaValue::String(s) if CROSSHAIR_RECOIL_MODES.contains(&s.as_str()) => Ok(()),
            other => Err(format!(
                "HUD override 'crosshair_recoil' must be true, false or one of {}, got {:?}",
                CROSSHAIR_RECOIL_MODES.join(", "),
                other
            )),
        },
        "crosshair_length" => ranged(CROSSHAIR_LENGTH_RANGE),
        "crosshair_thickness" => ranged(CROSSHAIR_THICKNESS_RANGE),
        "crosshair_gap" => ranged(CROSSHAIR_GAP_RANGE),
        "crosshair_outline" => ranged(CROSSHAIR_OUTLINE_RANGE),
        "crosshair_recoil_scale" => ranged(CROSSHAIR_RECOIL_SCALE_RANGE),
        "crosshair_dot" | "crosshair_t_style" => match value {
            LuaValue::Bool(_) => Ok(()),
            other => Err(format!(
                "HUD override '{key}' must be true or false, got {}",
                other.type_name()
            )),
        },
        _ => Err(format!(
            "Unknown HUD override key '{key}'; the keys are: {}",
            HUD_OVERRIDE_KEYS.join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_parse_with_and_without_alpha() {
        assert_eq!(parse_rgba_hex("00ff00"), Some([0, 255, 0, 255]));
        assert_eq!(parse_rgba_hex("#10203040"), Some([0x10, 0x20, 0x30, 0x40]));
        assert_eq!(parse_rgba_hex("0f0"), None);
        assert_eq!(parse_rgba_hex("zzzzzz"), None);
    }

    #[test]
    fn every_key_checks_its_value_and_unknown_keys_name_the_vocabulary() {
        let s = |t: &str| LuaValue::String(t.into());
        assert!(check_hud_override("crosshair", &s("@mod:a/assets/x.dtx")).is_ok());
        assert!(check_hud_override("hitmarker", &LuaValue::Number(1.0)).is_err());
        assert!(check_hud_override("crosshair_colour", &s("#00ff00")).is_ok());
        assert!(check_hud_override("crosshair_colour", &s("green")).is_err());
        assert!(check_hud_override("hud_scale", &LuaValue::Number(1.5)).is_ok());
        assert!(check_hud_override("hud_scale", &LuaValue::Number(2.5)).is_err());
        assert!(check_hud_override("crosshair_style", &s("circle_dot")).is_ok());
        assert!(check_hud_override("crosshair_style", &s("star")).is_err());
        assert!(check_hud_override("crosshair_recoil", &LuaValue::Bool(false)).is_ok());
        assert!(check_hud_override("crosshair_recoil", &s("no")).is_err());
        for m in CROSSHAIR_RECOIL_MODES {
            assert!(check_hud_override("crosshair_recoil", &s(m)).is_ok());
        }
        let n = LuaValue::Number;
        for (k, good, bad) in [
            ("crosshair_length", 7.0, -1.0),
            ("crosshair_thickness", 1.2, 0.0),
            ("crosshair_gap", -3.5, -51.0),
            ("crosshair_outline", 0.0, 11.0),
            ("crosshair_recoil_scale", 2.0, 6.0),
        ] {
            assert!(check_hud_override(k, &n(good)).is_ok(), "{k}");
            assert!(check_hud_override(k, &n(bad)).is_err(), "{k}");
            assert!(check_hud_override(k, &n(f64::NAN)).is_err(), "{k}");
            assert!(check_hud_override(k, &s("1")).is_err(), "{k}");
        }
        for k in ["crosshair_dot", "crosshair_t_style"] {
            assert!(check_hud_override(k, &LuaValue::Bool(true)).is_ok(), "{k}");
            assert!(check_hud_override(k, &n(1.0)).is_err(), "{k}");
        }
        let e = check_hud_override("radar", &s("x")).unwrap_err();
        assert!(
            e.contains("Unknown HUD override key 'radar'")
                && e.contains("crosshair, crosshair_colour"),
            "{e}"
        );
    }
}
