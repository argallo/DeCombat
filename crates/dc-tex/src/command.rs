//! Header command string. SPEC: FMT_DTX.md §7.
//!
//! Statements are lists of whitespace-separated tokens. The spec does not name a statement separator;
//! this parser accepts `;` and line breaks (DC-DEFAULT: tolerant reading, the ROM census only shows
//! which keywords occur).

use crate::DtxError;

/// Which material keyword matched. SPEC: FMT_DTX.md §7 table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterialKind {
    DetailTex,
    EnvMap,
    EnvMapAlpha,
    EnvBumpMap,
    EnvBumpMapNoFallback,
    Dot3BumpMap,
    Dot3EnvBumpMap,
    Effect,
}

impl MaterialKind {
    /// Mode number the engine assigns (1..=8). SPEC: FMT_DTX.md §7.
    pub fn mode(self) -> u8 {
        match self {
            MaterialKind::DetailTex => 1,
            MaterialKind::EnvMap => 2,
            MaterialKind::EnvMapAlpha => 3,
            MaterialKind::EnvBumpMap => 4,
            MaterialKind::EnvBumpMapNoFallback => 5,
            MaterialKind::Dot3BumpMap => 6,
            MaterialKind::Dot3EnvBumpMap => 7,
            MaterialKind::Effect => 8,
        }
    }
}

/// The secondary-texture directive of a texture (at most one is honoured by the engine).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterialDirective {
    pub kind: MaterialKind,
    /// Integer effect id (`Effect` only).
    pub effect_id: Option<i32>,
    /// Referenced texture names, in order (up to 4 for `Effect`).
    pub textures: Vec<String>,
}

/// Result of parsing a command string.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CommandSet {
    /// `AlphaRef` value, i16 range. SPEC: FMT_DTX.md §5, §7.
    pub alpha_test_value: Option<i16>,
    /// `MipMapBias` value.
    pub mip_bias: Option<f32>,
    /// First material keyword by the engine's test order.
    pub material: Option<MaterialDirective>,
}

/// (keyword, tokens required including the keyword, kind) in the engine's test order.
/// SPEC: FMT_DTX.md §7 (EVIDENCE: engine+0x89a60).
const MATERIAL_ORDER: [(&str, usize, MaterialKind); 8] = [
    ("DetailTex", 2, MaterialKind::DetailTex),
    ("EnvMap", 2, MaterialKind::EnvMap),
    ("EnvMapAlpha", 2, MaterialKind::EnvMapAlpha),
    ("EnvBumpMap", 3, MaterialKind::EnvBumpMap),
    (
        "EnvBumpMapNoFallback",
        3,
        MaterialKind::EnvBumpMapNoFallback,
    ),
    ("DOT3BumpMap", 2, MaterialKind::Dot3BumpMap),
    ("DOT3EnvBumpMap", 3, MaterialKind::Dot3EnvBumpMap),
    ("Effect", 2, MaterialKind::Effect),
];

/// Leading-integer reading (optional sign, digits), 0 when absent.
fn atoi(s: &str) -> i32 {
    let b = s.as_bytes();
    let (neg, mut i) = match b.first() {
        Some(b'-') => (true, 1),
        Some(b'+') => (false, 1),
        _ => (false, 0),
    };
    let mut v: i64 = 0;
    while let Some(c) = b.get(i).filter(|c| c.is_ascii_digit()) {
        v = (v * 10 + (c - b'0') as i64).min(i64::from(u32::MAX));
        i += 1;
    }
    (if neg { -v } else { v }).clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

/// Leading-float reading, 0.0 when absent.
fn atof(s: &str) -> f32 {
    let mut best = 0.0f32;
    for end in 1..=s.len() {
        if s.is_char_boundary(end) {
            if let Ok(v) = s[..end].parse::<f32>() {
                if v.is_finite() {
                    best = v;
                }
            }
        }
    }
    best
}

/// Parses a command string. Keywords are matched case-insensitively. A keyword with too few tokens does
/// not match for `AlphaRef`/`MipMapBias` (statement ignored); for material keywords it is
/// [`DtxError::CommandArgs`] ("invalid data" in the engine). Unknown statements are ignored.
pub fn parse_command(s: &str) -> Result<CommandSet, DtxError> {
    let statements: Vec<Vec<&str>> = s
        .split([';', '\n', '\r'])
        .map(|st| st.split_ascii_whitespace().collect::<Vec<_>>())
        .filter(|t| !t.is_empty())
        .collect();
    let mut set = CommandSet::default();
    for t in &statements {
        if t[0].eq_ignore_ascii_case("AlphaRef") && t.len() >= 2 && set.alpha_test_value.is_none() {
            set.alpha_test_value = Some(atoi(t[1]) as i16);
        } else if t[0].eq_ignore_ascii_case("MipMapBias") && t.len() >= 2 && set.mip_bias.is_none()
        {
            set.mip_bias = Some(atof(t[1]));
        }
    }
    for (kw, need, kind) in MATERIAL_ORDER {
        if let Some(t) = statements.iter().find(|t| t[0].eq_ignore_ascii_case(kw)) {
            if t.len() < need {
                return Err(DtxError::CommandArgs(kw));
            }
            let mut d = MaterialDirective {
                kind,
                effect_id: None,
                textures: Vec::new(),
            };
            if kind == MaterialKind::Effect {
                d.effect_id = Some(atoi(t[1]));
                d.textures = t[2..].iter().take(4).map(|x| x.to_string()).collect();
            } else {
                d.textures = t[1..]
                    .iter()
                    .take(need - 1)
                    .map(|x| x.to_string())
                    .collect();
            }
            set.material = Some(d);
            break;
        }
    }
    Ok(set)
}
