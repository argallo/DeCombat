//! `.fxf` effect file (CShell effect definitions): text and binary encodings.
//! SPEC: FMT_SPR_LTC_FX.md §5. Effect-type names are kept only as their sha-id (see `registry`).

use crate::error::{latin1_cstr, Bin, FxError};
use crate::registry::{name_id, FxType};
use crate::text::{Line, Lines};
use dc_rom::sha256_hex;
use std::collections::BTreeMap;

/// Most properties per key the loader reads. // EVIDENCE: cshell+0x29c4a0 (scratch 0x200)
pub const MAX_PROPS_PER_KEY: u32 = 0x200;
/// Our own bounds (the 2009 loader has none; the ROM maxima are 240 groups and 50 keys per group).
pub const MAX_GROUPS: u32 = 16384;
pub const MAX_KEYS_PER_GROUP: u32 = 4096;

const NAME_LEN: usize = 128;
const ATTACH_LEN: usize = 32;
/// Smallest binary key: type name, id, 40-byte link block, start, end, repeat, 3 ignored words, prop_count.
const MIN_KEY_BYTES: usize = NAME_LEN + 4 + 40 + 4 * 8;
const MIN_GROUP_BYTES: usize = 4 + NAME_LEN + 4;

/// sha256[:16] of the 7-byte text-encoding marker, compared case-insensitively. // EVIDENCE: cshell+0x29c560
const TEXT_MARKER_ID: &str = "914dc79e48187eee";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Text,
    Binary,
}

/// Case-insensitive test of the first 7 bytes against the marker, by hash: all 2^7 case variants of the
/// buffer are hashed (the marker text itself is not stored).
pub fn detect_encoding(data: &[u8]) -> Encoding {
    if data.len() < 7 {
        return Encoding::Binary;
    }
    let head = &data[..7];
    let letters: Vec<usize> = (0..7).filter(|&i| head[i].is_ascii_alphabetic()).collect();
    for mask in 0u32..(1 << letters.len()) {
        let mut v = head.to_vec();
        for (bit, &i) in letters.iter().enumerate() {
            v[i] = if mask >> bit & 1 == 1 {
                v[i].to_ascii_uppercase()
            } else {
                v[i].to_ascii_lowercase()
            };
        }
        if sha256_hex(&v)[..16] == *TEXT_MARKER_ID {
            return Encoding::Text;
        }
    }
    Encoding::Binary
}

/// The 8 property value types. // SPEC: FMT_SPR_LTC_FX.md §5.6
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PropType {
    String,
    Int,
    Float,
    Choice,
    Vec3,
    Vec4,
    ColourKey,
    Path,
    /// Any other type code: no value is read.
    Other(u32),
}

impl PropType {
    pub fn from_code(c: u32) -> PropType {
        match c {
            0 => PropType::String,
            1 => PropType::Int,
            2 => PropType::Float,
            3 => PropType::Choice,
            4 => PropType::Vec3,
            5 => PropType::Vec4,
            6 => PropType::ColourKey,
            7 => PropType::Path,
            o => PropType::Other(o),
        }
    }
    pub fn code(self) -> u32 {
        match self {
            PropType::String => 0,
            PropType::Int => 1,
            PropType::Float => 2,
            PropType::Choice => 3,
            PropType::Vec3 => 4,
            PropType::Vec4 => 5,
            PropType::ColourKey => 6,
            PropType::Path => 7,
            PropType::Other(o) => o,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    String(String),
    Int(i32),
    Float(f32),
    /// `raw` is `<index>,<option>,...` (cut at the first space in text files); `index` is its leading integer.
    Choice {
        index: i32,
        raw: String,
    },
    Vec3([f32; 3]),
    Vec4([f32; 4]),
    /// `t` plus colour packed as `(c4*255)<<24 | (c3*255)<<16 | (c2*255)<<8 | (c1*255)`.
    ColourKey {
        t: f32,
        packed: u32,
    },
    /// `<ext>|<archive path>`; see [`FieldValue::path_parts`].
    Path(String),
    None,
}

impl FieldValue {
    /// For a `Path` value: `(extension tag, path)`, with a path of three full stops reported as `None`.
    pub fn path_parts(&self) -> Option<(&str, Option<&str>)> {
        let FieldValue::Path(s) = self else {
            return None;
        };
        let mut it = s.split('|');
        let ext = it.next().unwrap_or("");
        let p = it.next().filter(|p| *p != "...");
        Some((ext, p))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    pub name: String,
    pub type_code: u32,
    pub value: FieldValue,
}

impl Property {
    pub fn prop_type(&self) -> PropType {
        PropType::from_code(self.type_code)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FxKey {
    /// Resolved registry type, if the name hashes to one of the 19.
    pub effect_type: Option<FxType>,
    /// sha256[:16] of the type name (cut at `;`): the registry id when resolved, else of the lowercased name.
    pub type_id: String,
    pub id: u32,
    pub linked: bool,
    pub link_id: u32,
    pub attach_node: String,
    /// Seconds, clamped to `[0, group length]`.
    pub start: f32,
    pub end: f32,
    pub repeat: u32,
    /// `end - start >= group length - 0.01`. // EVIDENCE: cshell+0x29c560
    pub whole_group: bool,
    pub properties: Vec<Property>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FxGroup {
    pub name: String,
    pub length_ms: u32,
    /// `length_ms * 0.001`
    pub length_s: f32,
    pub keys: Vec<FxKey>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FxfFile {
    pub encoding: Encoding,
    pub groups: Vec<FxGroup>,
    /// Bytes after the last group (text: 0 when only blanks remain).
    pub trailing: usize,
}

/// Totals for census checks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FxfCounts {
    pub groups: usize,
    pub keys: usize,
    pub properties: usize,
    pub props_by_type: BTreeMap<u32, usize>,
    pub keys_by_type: BTreeMap<usize, usize>,
    pub unresolved_keys: usize,
}

impl FxfFile {
    pub fn counts(&self) -> FxfCounts {
        let mut c = FxfCounts {
            groups: self.groups.len(),
            ..Default::default()
        };
        for g in &self.groups {
            for k in &g.keys {
                c.keys += 1;
                match k.effect_type {
                    Some(t) => *c.keys_by_type.entry(t.index()).or_default() += 1,
                    None => c.unresolved_keys += 1,
                }
                for p in &k.properties {
                    c.properties += 1;
                    *c.props_by_type.entry(p.type_code).or_default() += 1;
                }
            }
        }
        c
    }

    /// Parse either encoding, chosen by [`detect_encoding`]. SPEC: FMT_SPR_LTC_FX.md §5.2.
    pub fn parse(data: &[u8]) -> Result<FxfFile, FxError> {
        if data.is_empty() {
            return Err(FxError::Empty);
        }
        let enc = detect_encoding(data);
        Self::parse_as(data, enc)
    }

    pub fn parse_as(data: &[u8], enc: Encoding) -> Result<FxfFile, FxError> {
        let mut rd = match enc {
            Encoding::Text => Rd::Text(Lines::new(data)),
            Encoding::Binary => Rd::Bin(Bin::new(data)),
        };
        let groups_declared = rd.unsigned("group count")?;
        if groups_declared > MAX_GROUPS {
            return Err(FxError::TooMany {
                what: "groups",
                count: groups_declared,
            });
        }
        if let Rd::Bin(b) = &rd {
            if groups_declared as usize > b.remaining() / MIN_GROUP_BYTES {
                return Err(FxError::Truncated {
                    what: "groups",
                    offset: b.pos,
                });
            }
        }
        let mut groups = Vec::new();
        for _ in 0..groups_declared {
            groups.push(read_group(&mut rd)?);
        }
        // Whole-group flag, after all groups. // EVIDENCE: cshell+0x29c560
        for g in &mut groups {
            let thr = g.length_s - 0.01;
            for k in &mut g.keys {
                k.whole_group = k.end - k.start >= thr;
            }
        }
        let trailing = match &rd {
            Rd::Text(l) => {
                if l.only_blank_left() {
                    0
                } else {
                    l.remaining()
                }
            }
            Rd::Bin(b) => b.remaining(),
        };
        Ok(FxfFile {
            encoding: enc,
            groups,
            trailing,
        })
    }
}

enum Rd<'a> {
    Text(Lines<'a>),
    Bin(Bin<'a>),
}

impl<'a> Rd<'a> {
    fn line(&mut self, what: &'static str) -> Result<Line<'a>, FxError> {
        match self {
            Rd::Text(l) => l.next_line(what),
            Rd::Bin(_) => unreachable!("text line requested in binary mode"),
        }
    }
    fn unsigned(&mut self, what: &'static str) -> Result<u32, FxError> {
        match self {
            Rd::Text(l) => Ok(l.next_line(what)?.unsigned()),
            Rd::Bin(b) => b.u32(what),
        }
    }
    fn float(&mut self, what: &'static str) -> Result<f32, FxError> {
        match self {
            Rd::Text(l) => Ok(l.next_line(what)?.float()),
            Rd::Bin(b) => b.f32(what),
        }
    }
    /// Text: a word line. Binary: `char[128]`.
    fn name128(&mut self, what: &'static str) -> Result<String, FxError> {
        match self {
            Rd::Text(l) => Ok(cut(l.next_line(what)?.word(), NAME_LEN - 1)),
            Rd::Bin(b) => Ok(latin1_cstr(b.bytes(NAME_LEN, what)?, NAME_LEN)),
        }
    }
}

fn cut(s: String, max: usize) -> String {
    s.chars().take(max).collect()
}

fn read_group(rd: &mut Rd) -> Result<FxGroup, FxError> {
    let (name, key_count, length_ms);
    match rd {
        Rd::Text(_) => {
            name = rd.name128("group name")?;
            key_count = rd.unsigned("group key count")?;
            length_ms = rd.unsigned("group length")?;
        }
        Rd::Bin(_) => {
            key_count = rd.unsigned("group key count")?;
            name = rd.name128("group name")?;
            length_ms = rd.unsigned("group length")?;
        }
    }
    if key_count > MAX_KEYS_PER_GROUP {
        return Err(FxError::TooMany {
            what: "keys in group",
            count: key_count,
        });
    }
    if let Rd::Bin(b) = rd {
        if key_count as usize > b.remaining() / MIN_KEY_BYTES {
            return Err(FxError::Truncated {
                what: "keys",
                offset: b.pos,
            });
        }
    }
    let length_s = length_ms as f32 * 0.001;
    let mut keys = Vec::new();
    for _ in 0..key_count {
        keys.push(read_key(rd, length_s)?);
    }
    order_keys(&mut keys);
    Ok(FxGroup {
        name,
        length_ms,
        length_s,
        keys,
    })
}

fn clamp_time(v: f32, len: f32) -> f32 {
    v.max(0.0).min(len)
}

fn read_key(rd: &mut Rd, group_len: f32) -> Result<FxKey, FxError> {
    let raw_type = rd.name128("key type")?;
    let type_name = raw_type.split(';').next().unwrap_or("").to_string();
    let id = rd.unsigned("key id")?;
    let (linked, link_id, attach_node);
    match rd {
        Rd::Text(_) => {
            let l = rd.line("key linked")?.int();
            linked = (l as u8) != 0;
            link_id = rd.unsigned("key link_id")?;
            attach_node = cut(rd.line("key attach node")?.word(), ATTACH_LEN - 1);
        }
        Rd::Bin(b) => {
            let blk = b.bytes(40, "key link block")?;
            linked = blk[0] != 0;
            link_id = u32::from_le_bytes([blk[4], blk[5], blk[6], blk[7]]);
            attach_node = latin1_cstr(&blk[8..8 + ATTACH_LEN], ATTACH_LEN - 1);
        }
    }
    let start = clamp_time(rd.float("key start")?, group_len);
    let end = clamp_time(rd.float("key end")?, group_len);
    let repeat = rd.unsigned("key repeat")?;
    let _ = rd.unsigned("key ignored 1")?;
    let _ = rd.float("key ignored 2")?;
    let _ = rd.float("key ignored 3")?;
    let prop_count = rd.unsigned("key property count")?;
    if prop_count > MAX_PROPS_PER_KEY {
        return Err(FxError::TooMany {
            what: "properties in key",
            count: prop_count,
        });
    }
    let mut properties = Vec::new();
    for _ in 0..prop_count {
        properties.push(read_prop(rd)?);
    }
    let effect_type = FxType::resolve(&type_name);
    let type_id = match effect_type {
        Some(t) => t.id().to_string(),
        None => name_id(&type_name.to_ascii_lowercase()),
    };
    Ok(FxKey {
        effect_type,
        type_id,
        id,
        // A key linked to its own id is made unlinked. // EVIDENCE: cshell+0x29bed0
        linked: linked && link_id != id,
        link_id,
        attach_node,
        start,
        end,
        repeat,
        whole_group: false,
        properties,
    })
}

/// Leading decimal integer (C `atoi`) of the text before the first comma.
fn choice_index(raw: &str) -> i32 {
    let first = raw.split(',').next().unwrap_or("");
    let b = first.trim_start().as_bytes();
    let n = b
        .iter()
        .enumerate()
        .take_while(|(i, c)| c.is_ascii_digit() || (*i == 0 && (**c == b'-' || **c == b'+')))
        .count();
    scan_int_dec(&b[..n])
}

fn scan_int_dec(b: &[u8]) -> i32 {
    // decimal only (atoi), never octal/hex
    let (neg, d) = match b.first() {
        Some(b'-') => (true, &b[1..]),
        Some(b'+') => (false, &b[1..]),
        _ => (false, b),
    };
    let v = d.iter().fold(0i32, |a, c| {
        a.wrapping_mul(10).wrapping_add((c - b'0') as i32)
    });
    if neg {
        v.wrapping_neg()
    } else {
        v
    }
}

fn pack_colour(c: [f32; 4]) -> u32 {
    let q = |v: f32| (v * 255.0) as i32 as u32;
    (q(c[3]) << 24) | (q(c[2]) << 16) | (q(c[1]) << 8) | q(c[0])
}

fn read_prop(rd: &mut Rd) -> Result<Property, FxError> {
    match rd {
        Rd::Text(_) => {
            let name = cut(rd.line("property name")?.word(), NAME_LEN - 1);
            let type_code = rd.line("property type")?.int() as u32;
            let value = match PropType::from_code(type_code) {
                PropType::String => {
                    FieldValue::String(cut(rd.line("property value")?.word(), NAME_LEN - 1))
                }
                PropType::Int => FieldValue::Int(rd.line("property value")?.int()),
                PropType::Float => FieldValue::Float(rd.line("property value")?.float()),
                PropType::Choice => {
                    let raw = cut(rd.line("property value")?.word(), NAME_LEN - 1);
                    FieldValue::Choice {
                        index: choice_index(&raw),
                        raw,
                    }
                }
                PropType::Vec3 => FieldValue::Vec3(rd.line("property value")?.floats::<3>()),
                PropType::Vec4 => FieldValue::Vec4(rd.line("property value")?.floats::<4>()),
                PropType::ColourKey => {
                    let f = rd.line("property value")?.floats::<5>();
                    FieldValue::ColourKey {
                        t: f[0],
                        packed: pack_colour([f[1], f[2], f[3], f[4]]),
                    }
                }
                PropType::Path => {
                    FieldValue::Path(cut(rd.line("property value")?.word(), NAME_LEN - 1))
                }
                PropType::Other(_) => FieldValue::None,
            };
            Ok(Property {
                name,
                type_code,
                value,
            })
        }
        Rd::Bin(b) => {
            let n = b.u8("property name length")? as usize;
            let name = latin1_cstr(b.bytes(n, "property name")?, NAME_LEN - 1);
            let type_code = b.u32("property type")?;
            let value = match PropType::from_code(type_code) {
                PropType::String => {
                    FieldValue::String(latin1_cstr(b.bytes(NAME_LEN, "property value")?, NAME_LEN))
                }
                PropType::Int => FieldValue::Int(b.u32("property value")? as i32),
                PropType::Float => FieldValue::Float(b.f32("property value")?),
                PropType::Choice => {
                    let raw = latin1_cstr(b.bytes(NAME_LEN, "property value")?, NAME_LEN);
                    FieldValue::Choice {
                        index: choice_index(&raw),
                        raw,
                    }
                }
                PropType::Vec3 => FieldValue::Vec3([
                    b.f32("property value")?,
                    b.f32("property value")?,
                    b.f32("property value")?,
                ]),
                PropType::Vec4 => FieldValue::Vec4([
                    b.f32("property value")?,
                    b.f32("property value")?,
                    b.f32("property value")?,
                    b.f32("property value")?,
                ]),
                PropType::ColourKey => FieldValue::ColourKey {
                    t: b.f32("property value")?,
                    packed: b.u32("property value")?,
                },
                PropType::Path => {
                    FieldValue::Path(latin1_cstr(b.bytes(NAME_LEN, "property value")?, NAME_LEN))
                }
                PropType::Other(_) => FieldValue::None,
            };
            Ok(Property {
                name,
                type_code,
                value,
            })
        }
    }
}

/// Link targets move before the keys that link to them. SPEC: FMT_SPR_LTC_FX.md §5.4.
fn order_keys(keys: &mut [FxKey]) {
    let n = keys.len();
    for _ in 0..n {
        let mut swapped = false;
        for i in 0..n {
            if keys[i].linked {
                let target = keys[i].link_id;
                if let Some(j) = (i + 1..n).find(|&j| keys[j].id == target) {
                    keys.swap(i, j);
                    swapped = true;
                }
            }
        }
        if !swapped {
            break;
        }
    }
}
