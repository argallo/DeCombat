//! `ui_strings`: the localized UI text of the 2009 client, read from the string-table resources of the
//! user's `Game/CRes.dll` by numeric id, plus the cursor resources of the same image.
//!
//! PUBLIC: string table block layout (block `b` holds ids `16*(b-1) .. 16*(b-1)+15`; each slot is a u16
//! length in UTF-16 units followed by that many UTF-16LE units, length 0 = absent) and the `FormatMessage`
//! insert grammar (Win32 API reference). No Win32 dependency.
// SPEC: FMT_CRES_UI.md §2, §3.2

use crate::error::UiDataError;
use crate::pe_res::{
    resource_leaves, ResLeaf, ResName, RES_TYPE_CURSOR, RES_TYPE_GROUP_CURSOR, RES_TYPE_STRING,
};
use dc_rom::PeFile;
use std::collections::BTreeMap;
use std::path::Path;

/// Relative path of the resource image inside the client folder. SPEC: FMT_CRES_UI.md §2 (ALLOWED_SOURCES §4).
pub const CRES_REL_PATH: [&str; 2] = ["Game", "CRes.dll"];

/// One cursor image resource (PUBLIC: `RT_CURSOR` data starts with the u16 hotspot x, y).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorImage {
    pub id: u32,
    pub lang: u32,
    pub hotspot: (u16, u16),
    /// Bytes of the image after the hotspot (a DIB).
    pub size: u32,
}

/// One entry of a cursor group (PUBLIC: group header reserved/type/count, then 14-byte entries).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorGroupEntry {
    pub width: u16,
    pub height: u16,
    pub planes: u16,
    pub bit_count: u16,
    pub bytes: u32,
    /// Id of the [`CursorImage`] this entry names.
    pub image_id: u16,
}

/// A cursor group resource.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorGroup {
    pub id: u32,
    pub lang: u32,
    pub entries: Vec<CursorGroupEntry>,
}

/// The string table of the resource image, addressed by numeric id only.
#[derive(Clone, Debug, Default)]
pub struct UiStrings {
    strings: BTreeMap<u32, String>,
    blocks: usize,
    cursors: Vec<CursorImage>,
    cursor_groups: Vec<CursorGroup>,
    type_ids: Vec<u32>,
}

/// One argument of [`UiStrings::format`].
#[derive(Clone, Debug, PartialEq)]
pub enum FmtArg<'a> {
    Str(&'a str),
    Int(i64),
    UInt(u64),
}

impl UiStrings {
    /// Reads `Game/CRes.dll` of the client folder `rom`.
    pub fn from_rom(rom: &Path) -> Result<Self, UiDataError> {
        let path = rom.join(CRES_REL_PATH[0]).join(CRES_REL_PATH[1]);
        Self::from_pe(&PeFile::from_file(&path)?)
    }

    /// Reads the string blocks and cursors of an already loaded image.
    pub fn from_pe(pe: &PeFile) -> Result<Self, UiDataError> {
        let leaves = resource_leaves(pe)?;
        let mut out = UiStrings::default();
        for leaf in &leaves {
            if let ResName::Id(t) = leaf.kind {
                if !out.type_ids.contains(&t) {
                    out.type_ids.push(t);
                }
            }
        }
        // Several languages of one block would collide; the 2009 image has one (0x0409). Keep the first.
        let mut seen_blocks = std::collections::BTreeSet::new();
        for leaf in &leaves {
            let (Some(kind), Some(name)) = (leaf.kind.id(), leaf.name.id()) else {
                continue;
            };
            let data = pe.rva(leaf.data_rva, leaf.size as usize)?;
            let lang = leaf.lang.id().unwrap_or(0);
            match kind {
                RES_TYPE_STRING => {
                    if !seen_blocks.insert(name) {
                        continue;
                    }
                    out.blocks += 1;
                    for (id, s) in decode_string_block(name, data)? {
                        out.strings.insert(id, s);
                    }
                }
                RES_TYPE_CURSOR => out.cursors.push(decode_cursor(name, lang, data)?),
                RES_TYPE_GROUP_CURSOR => out
                    .cursor_groups
                    .push(decode_cursor_group(name, lang, data)?),
                _ => {}
            }
        }
        Ok(out)
    }

    /// The string with `id`, `None` when the image has none.
    pub fn get(&self, id: u32) -> Option<&str> {
        self.strings.get(&id).map(|s| s.as_str())
    }

    /// The string with `id`, or the empty string: a missing id is never an error (2009 behaviour, the
    /// accessors return an empty string). EVIDENCE: cshell+0x1a0420
    pub fn get_or_empty(&self, id: u32) -> &str {
        self.get(id).unwrap_or("")
    }

    /// Formats string `id` with the `FormatMessage` insert grammar; a missing id gives the empty string.
    /// EVIDENCE: cshell+0x1a0280
    pub fn format(&self, id: u32, args: &[FmtArg]) -> String {
        match self.get(id) {
            Some(s) => format_message(s, args),
            None => String::new(),
        }
    }

    /// Number of non-empty strings.
    pub fn len(&self) -> usize {
        self.strings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.strings.is_empty()
    }

    /// Number of string blocks read.
    pub fn string_block_count(&self) -> usize {
        self.blocks
    }

    /// All ids in ascending order.
    pub fn ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.strings.keys().copied()
    }

    /// Iterates `(id, string)` in id order.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &str)> {
        self.strings.iter().map(|(k, v)| (*k, v.as_str()))
    }

    /// The cursor images of the image.
    pub fn cursors(&self) -> &[CursorImage] {
        &self.cursors
    }

    /// The cursor groups of the image.
    pub fn cursor_groups(&self) -> &[CursorGroup] {
        &self.cursor_groups
    }

    /// Numeric resource type ids present in the image, in directory order.
    pub fn resource_types(&self) -> &[u32] {
        &self.type_ids
    }
}

/// Decodes one string block. `block` is the resource name id (1-based); returns `(string id, text)` for the
/// non-empty slots.
pub fn decode_string_block(block: u32, data: &[u8]) -> Result<Vec<(u32, String)>, UiDataError> {
    if block == 0 {
        return Err(UiDataError::Malformed("string block id 0".into()));
    }
    let first = (block - 1)
        .checked_mul(16)
        .ok_or_else(|| UiDataError::Malformed("string block id overflow".into()))?;
    let mut out = Vec::new();
    let mut p = 0usize;
    for slot in 0..16u32 {
        let Some(len_b) = data.get(p..p + 2) else {
            // Truncated trailing slots are absent.
            break;
        };
        let len = u16::from_le_bytes([len_b[0], len_b[1]]) as usize;
        p += 2;
        if len == 0 {
            continue;
        }
        let end = p + len * 2;
        let body = data
            .get(p..end)
            .ok_or_else(|| UiDataError::Malformed(format!("string block {block} slot {slot}")))?;
        let units: Vec<u16> = body
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        out.push((first + slot, String::from_utf16_lossy(&units)));
        p = end;
    }
    Ok(out)
}

fn decode_cursor(id: u32, lang: u32, data: &[u8]) -> Result<CursorImage, UiDataError> {
    if data.len() < 4 {
        return Err(UiDataError::Malformed(format!(
            "cursor {id} shorter than its hotspot"
        )));
    }
    Ok(CursorImage {
        id,
        lang,
        hotspot: (
            u16::from_le_bytes([data[0], data[1]]),
            u16::from_le_bytes([data[2], data[3]]),
        ),
        size: (data.len() - 4) as u32,
    })
}

fn decode_cursor_group(id: u32, lang: u32, data: &[u8]) -> Result<CursorGroup, UiDataError> {
    let u16_at =
        |o: usize| -> Option<u16> { data.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]])) };
    let malformed = || UiDataError::Malformed(format!("cursor group {id}"));
    let count = u16_at(4).ok_or_else(malformed)? as usize;
    let mut entries = Vec::with_capacity(count.min(256));
    for i in 0..count {
        let o = 6 + 14 * i;
        let e = data.get(o..o + 14).ok_or_else(malformed)?;
        let w = |k: usize| u16::from_le_bytes([e[k], e[k + 1]]);
        entries.push(CursorGroupEntry {
            width: w(0),
            height: w(2),
            planes: w(4),
            bit_count: w(6),
            bytes: u32::from_le_bytes([e[8], e[9], e[10], e[11]]),
            image_id: w(12),
        });
    }
    Ok(CursorGroup { id, lang, entries })
}

/// Leaves of `pe` of one resource type (diagnostics).
pub fn leaves_of_type(pe: &PeFile, kind: u32) -> Result<Vec<ResLeaf>, UiDataError> {
    Ok(resource_leaves(pe)?
        .into_iter()
        .filter(|l| l.kind == ResName::Id(kind))
        .collect())
}

// ---- FormatMessage insert grammar (PUBLIC: Win32 FormatMessage reference) ---------------------------------

/// Expands `%1`..`%99` (optionally `%n!printf-spec!`, default `!s!`) with `args`, and the escapes `%%`,
/// `%n` (CR LF), `%r`, `%t`, `%b` (space), `%.`, `%!`, `%0` (end of output). An insert that names a missing
/// argument expands to nothing; an unknown escape keeps the character after `%`.
pub fn format_message(src: &str, args: &[FmtArg]) -> String {
    let c: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < c.len() {
        if c[i] != '%' {
            out.push(c[i]);
            i += 1;
            continue;
        }
        i += 1;
        let Some(&e) = c.get(i) else {
            break;
        };
        match e {
            '0' => return out,
            '1'..='9' => {
                let mut n = e.to_digit(10).unwrap_or(0) as usize;
                i += 1;
                if let Some(d) = c.get(i).and_then(|d| d.to_digit(10)) {
                    n = n * 10 + d as usize;
                    i += 1;
                }
                let mut spec = String::from("s");
                if c.get(i) == Some(&'!') {
                    if let Some(end) = c[i + 1..].iter().position(|&x| x == '!') {
                        spec = c[i + 1..i + 1 + end].iter().collect();
                        i += end + 2;
                    }
                }
                if let Some(a) = n.checked_sub(1).and_then(|k| args.get(k)) {
                    out.push_str(&printf_one(&spec, a));
                }
            }
            'n' => {
                out.push_str("\r\n");
                i += 1;
            }
            'r' => {
                out.push('\r');
                i += 1;
            }
            't' => {
                out.push('\t');
                i += 1;
            }
            'b' => {
                out.push(' ');
                i += 1;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    out
}

/// One printf conversion (PUBLIC: C `printf` format spec): flags `-+ 0#`, width, `.precision`, length
/// modifiers `h l ll I64 w` (ignored), conversions `s S d i u x X o c`.
fn printf_one(spec: &str, arg: &FmtArg) -> String {
    let b: Vec<char> = spec.chars().collect();
    let mut i = 0;
    let (mut left, mut plus, mut space, mut zero, mut alt) = (false, false, false, false, false);
    while let Some(&f) = b.get(i) {
        match f {
            '-' => left = true,
            '+' => plus = true,
            ' ' => space = true,
            '0' => zero = true,
            '#' => alt = true,
            _ => break,
        }
        i += 1;
    }
    let mut width = 0usize;
    while let Some(d) = b.get(i).and_then(|c| c.to_digit(10)) {
        width = width
            .saturating_mul(10)
            .saturating_add(d as usize)
            .min(4096);
        i += 1;
    }
    let mut prec: Option<usize> = None;
    if b.get(i) == Some(&'.') {
        i += 1;
        let mut p = 0usize;
        while let Some(d) = b.get(i).and_then(|c| c.to_digit(10)) {
            p = p.saturating_mul(10).saturating_add(d as usize).min(4096);
            i += 1;
        }
        prec = Some(p);
    }
    // length modifiers
    loop {
        match b.get(i) {
            Some('h') | Some('l') | Some('w') | Some('L') => i += 1,
            Some('I') => {
                i += 1;
                let pair = (b.get(i), b.get(i + 1));
                if matches!(pair, (Some('6'), Some('4')) | (Some('3'), Some('2'))) {
                    i += 2;
                }
            }
            _ => break,
        }
    }
    let conv = b.get(i).copied().unwrap_or('s');
    let as_i64 = |a: &FmtArg| -> i64 {
        match a {
            FmtArg::Int(v) => *v,
            FmtArg::UInt(v) => *v as i64,
            FmtArg::Str(s) => s.trim().parse().unwrap_or(0),
        }
    };
    let as_u64 = |a: &FmtArg| -> u64 {
        match a {
            // A negative 32-bit value prints as its unsigned 32-bit pattern (the 2009 client is 32-bit).
            FmtArg::Int(v) if *v < 0 && *v >= i32::MIN as i64 => *v as i32 as u32 as u64,
            FmtArg::Int(v) => *v as u64,
            FmtArg::UInt(v) => *v,
            FmtArg::Str(s) => s.trim().parse().unwrap_or(0),
        }
    };
    let (sign, mut body, numeric) = match conv {
        'd' | 'i' => {
            let v = as_i64(arg);
            let sign = if v < 0 {
                "-"
            } else if plus {
                "+"
            } else if space {
                " "
            } else {
                ""
            };
            (sign, v.unsigned_abs().to_string(), true)
        }
        'u' => ("", as_u64(arg).to_string(), true),
        'x' => (
            if alt { "0x" } else { "" },
            format!("{:x}", as_u64(arg)),
            true,
        ),
        'X' => (
            if alt { "0X" } else { "" },
            format!("{:X}", as_u64(arg)),
            true,
        ),
        'o' => ("", format!("{:o}", as_u64(arg)), true),
        'c' => {
            let ch = match arg {
                FmtArg::Str(s) => s.chars().next().unwrap_or('\0'),
                other => char::from_u32(as_u64(other) as u32).unwrap_or('\u{fffd}'),
            };
            ("", ch.to_string(), false)
        }
        _ => {
            let s = match arg {
                FmtArg::Str(s) => s.to_string(),
                FmtArg::Int(v) => v.to_string(),
                FmtArg::UInt(v) => v.to_string(),
            };
            let s = match prec {
                Some(p) => s.chars().take(p).collect(),
                None => s,
            };
            ("", s, false)
        }
    };
    if numeric {
        if let Some(p) = prec {
            while body.chars().count() < p {
                body.insert(0, '0');
            }
        }
    }
    let len = sign.chars().count() + body.chars().count();
    if len >= width {
        return format!("{sign}{body}");
    }
    let pad = width - len;
    if left {
        format!("{sign}{body}{}", " ".repeat(pad))
    } else if zero && numeric && prec.is_none() {
        format!("{sign}{}{body}", "0".repeat(pad))
    } else {
        format!("{}{sign}{body}", " ".repeat(pad))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pe_res::{synth_pe_with_rsrc, synth_rsrc};

    fn block(slots: &[(usize, &str)]) -> Vec<u8> {
        let mut v = Vec::new();
        for i in 0..16 {
            match slots.iter().find(|(k, _)| *k == i) {
                Some((_, s)) => {
                    let u: Vec<u16> = s.encode_utf16().collect();
                    v.extend_from_slice(&(u.len() as u16).to_le_bytes());
                    for x in u {
                        v.extend_from_slice(&x.to_le_bytes());
                    }
                }
                None => v.extend_from_slice(&0u16.to_le_bytes()),
            }
        }
        v
    }

    fn cursor_group() -> Vec<u8> {
        let mut g = vec![0, 0, 2, 0, 1, 0];
        for w in [32u16, 64, 1, 1] {
            g.extend_from_slice(&w.to_le_bytes());
        }
        g.extend_from_slice(&300u32.to_le_bytes());
        g.extend_from_slice(&1u16.to_le_bytes());
        g
    }

    #[test]
    fn reads_synthetic_string_blocks_and_cursors() {
        let mut cur = vec![3, 0, 4, 0];
        cur.extend_from_slice(&[0u8; 40]);
        let sec = synth_rsrc(
            0x4000,
            &[
                (6, 1, 0x409, block(&[(0, "zero"), (15, "fifteen")])),
                (6, 5, 0x409, block(&[(2, "sixty-six %1!d! %2")])),
                (1, 1, 0x409, cur),
                (12, 7, 0x409, cursor_group()),
            ],
        );
        let pe = PeFile::from_bytes(synth_pe_with_rsrc(0x4000, &sec)).unwrap();
        let s = UiStrings::from_pe(&pe).unwrap();
        assert_eq!(s.string_block_count(), 2);
        assert_eq!(s.len(), 3);
        assert_eq!(s.get(0), Some("zero"));
        assert_eq!(s.get(15), Some("fifteen"));
        assert_eq!(s.get(66), Some("sixty-six %1!d! %2"));
        assert_eq!(s.get(1), None);
        assert_eq!(s.get_or_empty(1), "");
        assert_eq!(
            s.format(66, &[FmtArg::Int(-4), FmtArg::Str("x")]),
            "sixty-six -4 x"
        );
        assert_eq!(s.format(9999, &[]), "");
        assert_eq!(s.ids().collect::<Vec<_>>(), vec![0, 15, 66]);
        assert_eq!(s.cursors().len(), 1);
        assert_eq!(s.cursors()[0].hotspot, (3, 4));
        assert_eq!(s.cursors()[0].size, 40);
        assert_eq!(s.cursor_groups()[0].entries[0].image_id, 1);
        assert_eq!(s.cursor_groups()[0].entries[0].height, 64);
        assert_eq!(s.resource_types(), &[1, 6, 12]);
    }

    #[test]
    fn block_decoder_edges() {
        assert!(decode_string_block(0, &[]).is_err());
        assert!(decode_string_block(1, &[]).unwrap().is_empty());
        // length runs past the end
        assert!(decode_string_block(1, &[5, 0, b'a', 0]).is_err());
        // utf-16 surrogate pair
        let b = block(&[(1, "\u{1F600}")]);
        assert_eq!(
            decode_string_block(2, &b).unwrap(),
            vec![(17, "\u{1F600}".to_string())]
        );
    }

    #[test]
    fn format_message_grammar() {
        let a = [FmtArg::Str("ab"), FmtArg::UInt(255), FmtArg::Int(7)];
        assert_eq!(format_message("%1 and %2!x! %3!03d!", &a), "ab and ff 007");
        assert_eq!(format_message("100%% done%n", &a), "100% done\r\n");
        assert_eq!(format_message("a%tb%bc%.%!", &a), "a\tb c.!");
        assert_eq!(format_message("cut%0here", &a), "cut");
        assert_eq!(format_message("%9 missing", &a), " missing");
        assert_eq!(format_message("%2!-5u!|", &a), "255  |");
        assert_eq!(format_message("%1!5s!|%1!.1s!", &a), "   ab|a");
        assert_eq!(
            format_message("%3!+d! %2!#X! %2!lu! %2!I64d!", &a),
            "+7 0XFF 255 255"
        );
        assert_eq!(format_message("trailing %", &a), "trailing ");
        assert_eq!(format_message("%1!unterminated", &a), "ab!unterminated");
        assert_eq!(format_message("%12", &a), "");
        assert_eq!(
            format_message("%1!u! %1!x!", &[FmtArg::Int(-1)]),
            "4294967295 ffffffff"
        );
    }
}
