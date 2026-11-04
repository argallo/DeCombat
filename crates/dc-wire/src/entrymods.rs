//! A featured server entry's mod set on the wire (LG-313, project decision). Our own design, no 2009 counterpart.
//!
//! The login carries the LOBBY's hashes (LG-271) and comes before the player picks an entry, so an entry's set is
//! checked at the pick. All three frames are ours; the 2009 dispatcher drops ids it has no case for (SPEC:
//! PROTOCOL_2009.md §1.2), and a lobby without a featured entry set sends none of them.
//!
//! ```text
//! 0x3f10 entry_mods_push    (S->C, auth connection, before 0x2000)
//!   +6  u32 count (at most MAX_ENTRIES)
//!   +10 count x one NUL-terminated UTF-16LE line: entry id \t set hash \t content hash (or 0)
//! 0x3f11 entry_mods_claim   (C->S, auth or lobby connection, right before 0x20a / 0x3b2 of that entry)
//!   +6  u32 entry id
//!   +10 one NUL-terminated UTF-16LE line: set hash \t content hash (or 0)
//! 0x3f12 entry_mods_refusal (S->C, right before the refused 0x20b / 0x3b3)
//!   +6  u32 entry id
//!   +10 one NUL-terminated UTF-16LE line: the text for the player
//! ```
//!
//! The set itself (ids, versions, sha256) is the document `GET /entry_mods/<entry id>.json` on the lobby's web port, in
//! the form of `/server_list.json`. A reader skips broken lines and ignores further fields.
// DC-DEFAULT: LG-313 (proposed, project decision)

use crate::tcp::PktWriter;

pub const PUSH: u16 = crate::msg_ids::DC_ENTRY_MODS_PUSH;
pub const CLAIM: u16 = crate::msg_ids::DC_ENTRY_MODS_CLAIM;
pub const REFUSAL: u16 = crate::msg_ids::DC_ENTRY_MODS_REFUSAL;
/// Most entries one push carries.
pub const MAX_ENTRIES: usize = 32;
/// Longest line read or written (UTF-16 units).
pub const MAX_LINE_UNITS: usize = 512;

/// What one entry's set hashes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryHashes {
    pub entry: u32,
    pub set: String,
    pub content: String,
}

fn line(w: &mut PktWriter, text: &str) {
    for u in text.encode_utf16().filter(|&u| u != 0).take(MAX_LINE_UNITS) {
        w.u16(u);
    }
    w.u16(0);
}

fn read_line(d: &[u8], at: &mut usize) -> Option<String> {
    let mut units = Vec::new();
    loop {
        let b = d.get(*at..*at + 2)?;
        *at += 2;
        let u = u16::from_le_bytes([b[0], b[1]]);
        if u == 0 {
            return Some(String::from_utf16_lossy(&units));
        }
        units.push(u);
    }
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// The 0x3f10 frame.
pub fn push_frame(entries: &[EntryHashes]) -> Vec<u8> {
    let n = entries.len().min(MAX_ENTRIES);
    let mut w = PktWriter::new(PUSH);
    w.u32(n as u32);
    for e in &entries[..n] {
        line(&mut w, &format!("{}\t{}\t{}", e.entry, e.set, e.content));
    }
    w.into_frame()
}

/// Reads a 0x3f10 payload (the count at +6).
pub fn parse_push(d: &[u8]) -> Vec<EntryHashes> {
    let n = (u32_at(d, 6).unwrap_or(0) as usize).min(MAX_ENTRIES);
    let mut at = 10;
    let mut out = Vec::new();
    for _ in 0..n {
        let Some(l) = read_line(d, &mut at) else { break };
        let mut f = l.split('\t');
        if let (Some(Ok(entry)), Some(set), Some(content)) = (f.next().map(str::parse), f.next(), f.next()) {
            out.push(EntryHashes { entry, set: set.to_string(), content: content.to_string() });
        }
    }
    out
}

/// The 0x3f11 message.
pub fn claim_message(c: &EntryHashes) -> PktWriter {
    let mut w = PktWriter::new(CLAIM);
    w.u32(c.entry);
    line(&mut w, &format!("{}\t{}", c.set, c.content));
    w
}

/// Reads a 0x3f11 payload.
pub fn parse_claim(d: &[u8]) -> Option<EntryHashes> {
    let entry = u32_at(d, 6)?;
    let mut at = 10;
    let l = read_line(d, &mut at)?;
    let (set, content) = l.split_once('\t')?;
    let content = content.split('\t').next().unwrap_or_default();
    Some(EntryHashes { entry, set: set.to_ascii_lowercase(), content: content.to_ascii_lowercase() })
}

/// The 0x3f12 frame.
pub fn refusal_frame(entry: u32, text: &str) -> Vec<u8> {
    let mut w = PktWriter::new(REFUSAL);
    w.u32(entry);
    line(&mut w, text);
    w.into_frame()
}

/// Reads a 0x3f12 payload: the text.
pub fn parse_refusal(d: &[u8]) -> Option<String> {
    let mut at = 10;
    read_line(d, &mut at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_frames_round_trip_and_broken_lines_are_skipped() {
        let e = EntryHashes { entry: 3, set: "ab".repeat(32), content: "0".into() };
        let f = push_frame(std::slice::from_ref(&e));
        assert_eq!(parse_push(&f[8..]), vec![e.clone()]);
        let c = claim_message(&e).into_payload();
        assert_eq!(parse_claim(&c), Some(e));
        let r = refusal_frame(3, "wrong pack");
        assert_eq!(parse_refusal(&r[8..]).as_deref(), Some("wrong pack"));
        let mut w = PktWriter::new(PUSH);
        w.u32(2);
        line(&mut w, "not a line");
        line(&mut w, "4\ts\tc\textra");
        assert_eq!(parse_push(&w.into_payload()), vec![EntryHashes { entry: 4, set: "s".into(), content: "c".into() }]);
        assert_eq!(parse_claim(&[0; 6]), None);
    }
}
