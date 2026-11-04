//! The 19-type effect registry. SPEC: FMT_SPR_LTC_FX.md §6.3.
//!
//! Type names are never stored as text: the registry holds the first 16 hex digits of the SHA-256 of each
//! name (`sha256[:16]`) and its byte length, exactly as the spec lists them. Names read from effect files
//! are hashed and compared; the match is case-insensitive in the engine, so the lookup tries the plain
//! case variants (exact, lower, upper, first letter upper, each word capitalised).

use dc_rom::sha256_hex;

/// Our names for the 19 effect types, in registry order. Meanings are descriptive guesses (SPEC §6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum FxType {
    Particles = 0,
    Sprite,
    Empty,
    Model,
    DynamicLight,
    Sound,
    CameraShake,
    CameraWobble,
    DebrisChunk,
    ModelDebrisChunk,
    FallingParticles,
    Trail,
    RandomSound,
    SpriteSystem,
    SpawnGroup,
    FlareSprite,
    Lightning,
    ModelSystem,
    DamageSystem,
}

pub const TYPE_COUNT: usize = 19;

/// `(sha256[:16] of the name, name length)` per type, in registry order. // SPEC: FMT_SPR_LTC_FX.md §6.3
pub const TYPE_IDS: [(&str, usize); TYPE_COUNT] = [
    ("8c7cd19b782c034e", 14),
    ("610cc33146e5be61", 6),
    ("fa1d2db62d4d952e", 4),
    ("05d155e975caebb4", 8),
    ("3a7c035a9d047515", 9),
    ("d9dfcd3d1511b47f", 9),
    ("2e632023d520b1c5", 9),
    ("31c339c4f400a354", 7),
    ("722697335a33c224", 11),
    ("aec8312c467542ee", 14),
    ("e0bd461d6454d556", 12),
    ("37d3a5d2172ecf93", 9),
    ("e62daa0961f3a3b7", 15),
    ("d7f4686854dc7a3d", 12),
    ("c05f736cea8ca9b1", 8),
    ("470cd96d88c2fe56", 13),
    ("77425462fb006b12", 11),
    ("ff5af605d800f0ce", 11),
    ("81fcc021a4d9930c", 14),
];

/// `type_flags` per type. // SPEC: FMT_SPR_LTC_FX.md §6.3 (meaning unproven)
pub const TYPE_FLAGS: [u32; TYPE_COUNT] = [
    0x17, 0x17, 0x14, 0x17, 0x17, 0x1c, 0x22, 0x22, 0x13, 0x13, 0x37, 0x17, 0x14, 0x17, 0, 0x11,
    0x17, 0x17, 0,
];

const ALL: [FxType; TYPE_COUNT] = [
    FxType::Particles,
    FxType::Sprite,
    FxType::Empty,
    FxType::Model,
    FxType::DynamicLight,
    FxType::Sound,
    FxType::CameraShake,
    FxType::CameraWobble,
    FxType::DebrisChunk,
    FxType::ModelDebrisChunk,
    FxType::FallingParticles,
    FxType::Trail,
    FxType::RandomSound,
    FxType::SpriteSystem,
    FxType::SpawnGroup,
    FxType::FlareSprite,
    FxType::Lightning,
    FxType::ModelSystem,
    FxType::DamageSystem,
];

impl FxType {
    pub fn from_index(i: usize) -> Option<FxType> {
        ALL.get(i).copied()
    }
    pub fn index(self) -> usize {
        self as usize
    }
    pub fn all() -> &'static [FxType; TYPE_COUNT] {
        &ALL
    }
    /// sha256[:16] id of the type name.
    pub fn id(self) -> &'static str {
        TYPE_IDS[self.index()].0
    }
    pub fn type_flags(self) -> u32 {
        TYPE_FLAGS[self.index()]
    }

    /// Resolve a name from an effect file (already cut at `;`).
    pub fn resolve(name: &str) -> Option<FxType> {
        if name.is_empty() || name.len() > 127 {
            return None;
        }
        for cand in case_variants(name) {
            if let Some(t) = Self::from_name_exact(&cand) {
                return Some(t);
            }
        }
        None
    }

    fn from_name_exact(name: &str) -> Option<FxType> {
        let h = name_id(name);
        TYPE_IDS
            .iter()
            .position(|(id, len)| *id == h && *len == name.len())
            .and_then(FxType::from_index)
    }
}

/// `sha256[:16]` of the exact bytes of a name (Latin-1 chars map to single bytes).
pub fn name_id(name: &str) -> String {
    let bytes: Vec<u8> = name.chars().map(|c| c as u32 as u8).collect();
    sha256_hex(&bytes)[..16].to_string()
}

fn case_variants(name: &str) -> Vec<String> {
    let lower = name.to_ascii_lowercase();
    let mut first = lower.clone();
    if let Some(c) = first.get_mut(0..1) {
        c.make_ascii_uppercase();
    }
    let mut words = String::with_capacity(lower.len());
    let mut up = true;
    for ch in lower.chars() {
        words.push(if up { ch.to_ascii_uppercase() } else { ch });
        up = ch == ' ' || ch == '_';
    }
    vec![
        name.to_string(),
        lower.clone(),
        name.to_ascii_uppercase(),
        first,
        words,
    ]
}
