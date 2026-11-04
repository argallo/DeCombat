//! Object list (section 6): class name plus typed properties.

use crate::cursor::{le_f32, le_vec3, Cur, Vec3};
use crate::error::{Result, Section, WorldError};
use crate::names::Key;
use crate::roles::{ObjectRole, PropKey};

/// Property type code as stored in the file.
// SPEC: FMT_DAT85.md §6 (property types; jump table engine+0x946a4, dispatch engine+0x9434c)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PropKind {
    Text,
    Vec3,
    Rgb,
    Real,
    Bits,
    Bool,
    Int,
    Euler,
    /// Codes above 7: skipped by the engine (engine+0x9434f).
    Other(u8),
}

impl PropKind {
    pub fn from_code(c: u8) -> Self {
        match c {
            0 => PropKind::Text,
            1 => PropKind::Vec3,
            2 => PropKind::Rgb,
            3 => PropKind::Real,
            4 => PropKind::Bits,
            5 => PropKind::Bool,
            6 => PropKind::Int,
            7 => PropKind::Euler,
            n => PropKind::Other(n),
        }
    }

    pub fn code(self) -> u8 {
        match self {
            PropKind::Text => 0,
            PropKind::Vec3 => 1,
            PropKind::Rgb => 2,
            PropKind::Real => 3,
            PropKind::Bits => 4,
            PropKind::Bool => 5,
            PropKind::Int => 6,
            PropKind::Euler => 7,
            PropKind::Other(n) => n,
        }
    }
}

/// Decoded property value.
// SPEC: FMT_DAT85.md §6 (property types; conversions engine+0x92a40..0x92d00)
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FieldValue<'a> {
    /// SPEC: FMT_DAT85.md §6 code 0 (engine+0x9436c -> 0x92b00)
    Text(&'a [u8]),
    /// SPEC: FMT_DAT85.md §6 code 1
    Vec3(Vec3),
    /// SPEC: FMT_DAT85.md §6 code 2 (3 x f32)
    Rgb(Vec3),
    /// SPEC: FMT_DAT85.md §6 code 3
    Real(f32),
    /// SPEC: FMT_DAT85.md §6 code 4: f32 on disk, truncated toward zero (cvttss2si at engine+0x9439e)
    Bits(i32),
    /// SPEC: FMT_DAT85.md §6 code 5: one byte, nonzero = true (engine+0x943be)
    Bool(bool),
    /// SPEC: FMT_DAT85.md §6 code 6: f32 on disk, truncated toward zero
    Int(i32),
    /// SPEC: FMT_DAT85.md §6 code 7: 16 bytes, three radians used (engine+0x943e8), fourth f32 unused
    Euler([f32; 4]),
    /// Unknown code, or a payload too short for its code: the raw bytes.
    Raw(&'a [u8]),
}

/// One property.
// SPEC: FMT_DAT85.md §6 (property record)
#[derive(Debug, Clone, PartialEq)]
pub struct Property<'a> {
    pub name: &'a [u8],
    pub kind: PropKind,
    /// Editor flags, kept opaque (FMT_DAT85.md §13 item 3).
    pub flags: u32,
    /// `prop_len` as declared.
    pub declared_len: u16,
    /// The payload bytes exactly as stored (for text: the inner bytes, without the u16 count).
    pub raw: &'a [u8],
    pub value: FieldValue<'a>,
}

/// f32 to integer with truncation toward zero; Rust's `as` saturates and maps NaN to 0, so hostile values
/// cannot panic.
// SPEC: FMT_DAT85.md §12 (types 4 and 6 truncate toward zero)
fn trunc(v: f32) -> i32 {
    v as i32
}

fn decode_value(kind: PropKind, raw: &[u8]) -> FieldValue<'_> {
    match kind {
        PropKind::Text => FieldValue::Text(raw),
        PropKind::Vec3 if raw.len() >= 12 => FieldValue::Vec3(le_vec3(raw, 0)),
        PropKind::Rgb if raw.len() >= 12 => FieldValue::Rgb(le_vec3(raw, 0)),
        PropKind::Real if raw.len() >= 4 => FieldValue::Real(le_f32(raw, 0)),
        PropKind::Bits if raw.len() >= 4 => FieldValue::Bits(trunc(le_f32(raw, 0))),
        PropKind::Bool if !raw.is_empty() => FieldValue::Bool(raw[0] != 0),
        PropKind::Int if raw.len() >= 4 => FieldValue::Int(trunc(le_f32(raw, 0))),
        PropKind::Euler if raw.len() >= 16 => FieldValue::Euler([
            le_f32(raw, 0),
            le_f32(raw, 4),
            le_f32(raw, 8),
            le_f32(raw, 12),
        ]),
        _ => FieldValue::Raw(raw),
    }
}

/// One object of the object list.
// SPEC: FMT_DAT85.md §6 (object record)
#[derive(Debug, Clone, PartialEq)]
pub struct Object<'a> {
    pub class_name: &'a [u8],
    /// Role from the class-name hash (FMT_DAT85.md §6.2), if the class is one we label.
    pub role: Option<ObjectRole>,
    /// Whether the class is one of DeCombat's own `dc.*` names ([`crate::names::is_dc_class`], set at parse
    /// time): only then do the property lookups by key ([`Object::prop_named`], [`Object::prop_by_key`] and the
    /// typed reads over them) accept the own property names; a ROM object's properties resolve by hash alone.
    // DC-DEFAULT: MAP-2
    pub dc: bool,
    pub props: Vec<Property<'a>>,
    /// File offset of the `obj_len` field.
    pub at: usize,
}

impl<'a> Object<'a> {
    /// First property whose name equals `name`.
    pub fn prop(&self, name: &[u8]) -> Option<&Property<'a>> {
        self.props.iter().find(|p| p.name == name)
    }

    /// First property whose name is `key`: the ROM name by hash and length, or (on a `dc.*` object only) the
    /// DeCombat name, ASCII case ignored. The three leading properties are better read by index
    /// ([`Object::name`], [`Object::position`], [`Object::rotation`]).
    // DC-DEFAULT: MAP-2
    pub fn prop_by_key(&self, key: PropKey) -> Option<&Property<'a>> {
        self.prop_named(key.key())
    }

    /// First property whose name matches `key`: the alias-aware replacement for the per-crate
    /// `prop(o, PropHash)` helpers. The ROM half ([`Key::matches_hash`]) applies on every object; the own-name
    /// half ([`Key::matches_ours`]) only when the object's class is a `dc.*` name ([`Object::dc`]), so a ROM
    /// property that spells a vocabulary word on a ROM object is never taken for the vocabulary key.
    // DC-DEFAULT: MAP-2
    pub fn prop_named(&self, key: &Key) -> Option<&Property<'a>> {
        self.props
            .iter()
            .find(|p| key.matches_hash(p.name) || (self.dc && key.matches_ours(p.name)))
    }

    /// Whether the object's class is `key` ([`Key::matches`]: hash or own name on any object), for classes
    /// beyond the roles (`names::CLASS_*`).
    pub fn is_class(&self, key: &Key) -> bool {
        key.matches(self.class_name)
    }

    pub fn text(&self, key: PropKey) -> Option<&'a [u8]> {
        match self.prop_by_key(key)?.value {
            FieldValue::Text(t) => Some(t),
            _ => None,
        }
    }

    pub fn real(&self, key: PropKey) -> Option<f32> {
        match self.prop_by_key(key)?.value {
            FieldValue::Real(v) => Some(v),
            _ => None,
        }
    }

    pub fn vec3(&self, key: PropKey) -> Option<Vec3> {
        match self.prop_by_key(key)?.value {
            FieldValue::Vec3(v) | FieldValue::Rgb(v) => Some(v),
            _ => None,
        }
    }

    /// Object name: property 0 (text), the link key to a BSP model of the same name.
    // SPEC: FMT_DAT85.md §6.1 index 0, §6.4
    pub fn name(&self) -> Option<&'a [u8]> {
        match self.props.first()?.value {
            FieldValue::Text(t) => Some(t),
            _ => None,
        }
    }

    /// Position: property 1 (vec3).
    // SPEC: FMT_DAT85.md §6.1 index 1
    pub fn position(&self) -> Option<Vec3> {
        match self.props.get(1)?.value {
            FieldValue::Vec3(v) => Some(v),
            _ => None,
        }
    }

    /// Orientation: property 2 (euler, three radians).
    // SPEC: FMT_DAT85.md §6.1 index 2
    pub fn rotation(&self) -> Option<Vec3> {
        match self.props.get(2)?.value {
            FieldValue::Euler([x, y, z, _]) => Some([x, y, z]),
            _ => None,
        }
    }
}

pub(crate) fn read_objects<'a>(c: &mut Cur<'a>) -> Result<Vec<Object<'a>>> {
    c.section = Section::Objects;
    // SPEC: FMT_DAT85.md §6 item 1 (engine+0x940d3)
    let n = c.u32()?;
    let mut out = Vec::with_capacity(c.cap(n, 8));
    for i in 0..n {
        let at = c.pos();
        // SPEC: FMT_DAT85.md §6 object items 1-4 (engine+0x940ef, 0x9410f, 0x941e6)
        let obj_len = c.u16()?;
        let start = c.pos();
        let class_name = c.lstr()?;
        let n_props = c.u32()?;
        let mut props = Vec::with_capacity(c.cap(n_props, 9));
        for _ in 0..n_props {
            // SPEC: FMT_DAT85.md §6 property items 1-5 (engine+0x94231 .. 0x94330)
            let name = c.lstr()?;
            let kind = PropKind::from_code(c.u8()?);
            let flags = c.u32()?;
            let declared_len = c.u16()?;
            let raw = match kind {
                PropKind::Text => c.lstr()?,
                _ => c.take(declared_len as usize)?,
            };
            props.push(Property {
                name,
                kind,
                flags,
                declared_len,
                raw,
                value: decode_value(kind, raw),
            });
        }
        let actual = c.pos() - start;
        if actual != obj_len as usize {
            return Err(WorldError::ObjectLength {
                object: i,
                declared: obj_len,
                actual,
            });
        }
        out.push(Object {
            class_name,
            role: ObjectRole::of_class(class_name),
            dc: crate::names::is_dc_class(class_name),
            props,
            at,
        });
    }
    Ok(out)
}
