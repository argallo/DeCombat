//! The other lobbies a lobby lists after its own server list (LG-272, project decision). Our own design, no
//! 2009 counterpart.
//!
//! **Carrier.** The 2009 server list 0x2000 is a fixed array of 0xc0-byte entries (20 at most, the client copies
//! 0xf16 bytes) whose address field is the entry's own lobby connection: it has no room for a second lobby's
//! address, its certificate pin, its mod policy or a region. So a lobby with `[[lobby.servers]]` entries that name
//! ANOTHER lobby instance (`lobby = "host:port"`) sends our own push **0x3f02** on the auth connection right BEFORE
//! 0x2000. The 2009 dispatcher drops ids it has no case for (SPEC: PROTOCOL_2009.md §1.2, LIVE for 0x3f01), so the
//! 2009 client sees only the plain entries; our client keeps the frame and reads it once 0x2000 has arrived. A
//! lobby without such entries sends nothing, so its frames stay as before.
//!
//! ```text
//! 0x3f02 modded_servers_push  (S->C, auth connection, before 0x2000)
//!   +6  u32 count (at most MAX_ENTRIES)
//!   +10 count x one NUL-terminated UTF-16LE line of tab-separated fields:
//!       name \t region \t lobby (host:port) \t fingerprint (64 hex or empty) \t policy (modded | vanilla)
//! ```
//!
//! Fields never contain a tab or a NUL (the writer replaces them with a space); a line is cut at
//! [`MAX_LINE_UNITS`]. A reader ignores lines with fewer than five fields or an empty name or lobby, and fields after
//! the fifth (room for later additions).
// DC-DEFAULT: LG-272 (proposed, project decision) the modded rows of the post-login server list

use crate::tcp::PktWriter;

/// Our push id (not a 2009 id).
pub const MODDED_SERVERS_PUSH: u16 = crate::msg_ids::DC_MODDED_SERVERS_PUSH;
/// Most entries one push carries.
pub const MAX_ENTRIES: usize = 32;
/// Longest line (UTF-16 units).
pub const MAX_LINE_UNITS: usize = 512;

/// One other lobby of the list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListedLobby {
    /// Shown on the row.
    pub name: String,
    /// Shown on the row ("" = none).
    pub region: String,
    /// `host:port` of the lobby's auth port: data only, never shown.
    pub lobby: String,
    /// The SHA-256 of the lobby's certificate (64 hex digits) or "" (the client's own pin rules).
    pub fingerprint: String,
    /// `modded` or `vanilla`.
    pub policy: String,
}

fn clean(s: &str) -> String {
    s.replace(['\t', '\0', '\n', '\r'], " ").trim().to_string()
}

/// The 0x3f02 frame for `entries` (the first [`MAX_ENTRIES`]).
pub fn frame(entries: &[ListedLobby]) -> Vec<u8> {
    let n = entries.len().min(MAX_ENTRIES);
    let mut w = PktWriter::new(MODDED_SERVERS_PUSH);
    w.u32(n as u32);
    for e in &entries[..n] {
        let line = [&e.name, &e.region, &e.lobby, &e.fingerprint, &e.policy]
            .iter()
            .map(|f| clean(f))
            .collect::<Vec<_>>()
            .join("\t");
        for u in line.encode_utf16().take(MAX_LINE_UNITS) {
            w.u16(u);
        }
        w.u16(0);
    }
    w.into_frame()
}

/// Reads a 0x3f02 payload (inner header included: the count is at +6).
pub fn parse(d: &[u8]) -> Vec<ListedLobby> {
    let u32_at = |o: usize| {
        d.get(o..o + 4)
            .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let n = (u32_at(6) as usize).min(MAX_ENTRIES);
    let mut at = 10usize;
    let mut out = Vec::new();
    for _ in 0..n {
        let mut units = Vec::new();
        while let Some(b) = d.get(at..at + 2) {
            at += 2;
            let u = u16::from_le_bytes([b[0], b[1]]);
            if u == 0 {
                break;
            }
            units.push(u);
        }
        let line = String::from_utf16_lossy(&units);
        let f: Vec<&str> = line.split('\t').map(str::trim).collect();
        if f.len() < 5 || f[0].is_empty() || f[2].is_empty() {
            continue;
        }
        out.push(ListedLobby {
            name: f[0].to_string(),
            region: f[1].to_string(),
            lobby: f[2].to_string(),
            fingerprint: f[3].to_string(),
            policy: f[4].to_string(),
        });
        if at >= d.len() {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_push_round_trips_and_skips_broken_lines() {
        let e = vec![
            ListedLobby {
                name: "US Central Modded".into(),
                region: "na".into(),
                lobby: "203.0.113.7:11000".into(),
                fingerprint: "ab".repeat(32),
                policy: "modded".into(),
            },
            ListedLobby {
                name: "Tab\tName".into(),
                region: String::new(),
                lobby: "example.org:12000".into(),
                fingerprint: String::new(),
                policy: "modded".into(),
            },
        ];
        let f = frame(&e);
        assert_eq!(u16::from_le_bytes([f[8], f[9]]), MODDED_SERVERS_PUSH);
        let back = parse(&f[8..]);
        assert_eq!(back[0], e[0]);
        assert_eq!(back[1].name, "Tab Name");
        assert_eq!(back[1].lobby, "example.org:12000");
        // a line without a lobby is skipped
        let bad = frame(&[ListedLobby {
            name: "x".into(),
            ..ListedLobby::default()
        }]);
        assert!(parse(&bad[8..]).is_empty());
    }
}
