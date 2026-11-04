//! Offline reader of a captap capture (format 1): every UDP game connection's datagrams are replayed, one
//! direction at a time, into a receiving `dc_net_engine::Link`, which hands back the session messages in the
//! order the receiver would have seen them (reliable messages once and in order, resends dropped, split messages
//! joined). Used by the `capdecode` example and by the shooter analysis; it never sends anything.
//!
//! Captures are evidence only (`REFERENCE_TESTING.md` §1, §5): this module reads them at run time and nothing from a
//! capture enters the tree.
// SPEC: UDP_TRANSPORT_2009.md §4 (out-of-band vs link datagrams), §6-§10 (link blocks, reliable order, split)
// SPEC: REFERENCE_TESTING.md §3 (capture format 1); HARNESS: captap.py (format 1 lines)

use std::collections::BTreeMap;
use std::io::BufRead;

use dc_net_engine::{Link, LinkConfig, LinkEvent, Millis};
use dc_wire::udp::{is_oob_datagram, BitWriter};
use serde_json::Value;

use crate::shell::{c2s, s2c};

/// One delivered session message of a capture.
#[derive(Debug, Clone)]
pub struct CapMsg {
    /// Capture time of the datagram that completed the message (seconds).
    pub t: f64,
    /// Capture line `seq` of the datagram that completed the message.
    // OURS: capture format 1 field (capture README.md)
    pub seq: u64,
    pub conn: u32,
    /// Client -> server.
    pub c2s: bool,
    pub reliable: bool,
    pub bits: BitWriter,
}

impl CapMsg {
    /// The engine message id.
    pub fn id(&self) -> u8 {
        self.bits.as_bytes().first().copied().unwrap_or(0)
    }

    /// The shell id when this is a shell message in its direction.
    pub fn shell(&self) -> Option<u8> {
        let sm = if self.c2s {
            c2s::SHELL_MSG
        } else {
            s2c::SHELL_MSG
        };
        (self.id() == sm).then(|| self.bits.as_bytes().get(1).copied().unwrap_or(0))
    }
}

/// A UDP connection of the capture.
#[derive(Debug, Clone, Default)]
pub struct CapConn {
    pub conn: u32,
    /// The client's address as the tap saw it.
    pub peer: String,
    pub c2s_datagrams: u64,
    pub s2c_datagrams: u64,
    pub first_t: Option<f64>,
}

/// Everything the decoder found.
#[derive(Debug, Clone, Default)]
pub struct Decoded {
    pub conns: BTreeMap<u32, CapConn>,
    pub msgs: Vec<CapMsg>,
    /// TCP frames: (t, conn, c2s, msg_id, bytes).
    pub tcp: Vec<(f64, u32, bool, u16, Vec<u8>)>,
}

impl Decoded {
    /// Messages of one connection and direction.
    pub fn of(&self, conn: u32, c2s: bool) -> impl Iterator<Item = &CapMsg> {
        self.msgs
            .iter()
            .filter(move |m| m.conn == conn && m.c2s == c2s)
    }

    /// The UDP connection whose client address ends with `:port`.
    pub fn conn_by_port(&self, port: u16) -> Option<u32> {
        let suffix = format!(":{port}");
        self.conns
            .values()
            .find(|c| c.peer.ends_with(&suffix))
            .map(|c| c.conn)
    }
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .filter_map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok())
        .collect()
}

/// Decodes a capture from its lines.
pub fn decode<R: BufRead>(input: R) -> Decoded {
    let mut out = Decoded::default();
    // one receiving link per (conn, direction)
    let mut links: BTreeMap<(u32, bool), Link> = BTreeMap::new();
    for line in input.lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let ev = v["ev"].as_str().unwrap_or("");
        let conn = v["conn"].as_u64().unwrap_or(0) as u32;
        let t = v["t"].as_f64().unwrap_or(0.0);
        let seq = v["seq"].as_u64().unwrap_or(0);
        let proto = v["proto"].as_str().unwrap_or("");
        match (ev, proto) {
            ("open", "udp") => {
                out.conns.insert(
                    conn,
                    CapConn {
                        conn,
                        peer: v["peer"].as_str().unwrap_or("").to_string(),
                        ..CapConn::default()
                    },
                );
            }
            ("frame", "tcp") => {
                let bytes = unhex(v["hex"].as_str().unwrap_or(""));
                let id = v["msg_id"].as_u64().unwrap_or(0) as u16;
                out.tcp
                    .push((t, conn, v["dir"].as_str() == Some("c2s"), id, bytes));
            }
            ("dgram", "udp") => {
                let up = v["dir"].as_str() == Some("c2s");
                let bytes = unhex(v["hex"].as_str().unwrap_or(""));
                let c = out.conns.entry(conn).or_insert_with(|| CapConn {
                    conn,
                    ..CapConn::default()
                });
                c.first_t.get_or_insert(t);
                if up {
                    c.c2s_datagrams += 1;
                } else {
                    c.s2c_datagrams += 1;
                }
                if is_oob_datagram(&bytes) {
                    continue;
                }
                let now = (t * 1000.0) as Millis;
                let link = links.entry((conn, up)).or_insert_with(|| {
                    Link::new(
                        now,
                        LinkConfig {
                            // the receiver of client datagrams is the host
                            hosting: up,
                            ..LinkConfig::default()
                        },
                    )
                });
                link.handle_datagram(now, &bytes);
                for e in link.poll(now) {
                    if let LinkEvent::Message { reliable, bits } = e {
                        out.msgs.push(CapMsg {
                            t,
                            seq,
                            conn,
                            c2s: up,
                            reliable,
                            bits,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Decodes a capture file.
pub fn decode_file(path: &std::path::Path) -> std::io::Result<Decoded> {
    let f = std::fs::File::open(path)?;
    Ok(decode(std::io::BufReader::new(f)))
}

/// Splits a server bundle (engine id 14) into its inner messages; any other message comes back alone.
// SPEC: UDP_SESSION_2009_A.md §3.4; addendum A20 (entries `[len:u8 bits][bits]`, len 0 ends)
pub fn unbundle(bits: &BitWriter) -> Vec<BitWriter> {
    let mut r = bits.reader();
    if bits.as_bytes().first() != Some(&s2c::BUNDLE) {
        return vec![bits.clone()];
    }
    r.read_u8();
    let mut v = Vec::new();
    while r.remaining_bits() >= 8 {
        let len = r.read_u8() as usize;
        if len == 0 || len > r.remaining_bits() {
            break;
        }
        v.extend(unbundle(&r.read_bit_string(len)));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_wire::udp::BitWriter;

    #[test]
    fn link_replay_returns_reliable_messages_once() {
        // a sender link produces datagrams; the decoder sees them twice (a resend) and delivers each message once
        let mut tx = Link::new(0, LinkConfig::default());
        let mut m = BitWriter::new();
        m.write_u8(c2s::SHELL_MSG).write_u8(0x7f).write_u8(1);
        tx.send_reliable(m.clone());
        let mut lines = String::from(
            "{\"ev\":\"open\",\"conn\":3,\"proto\":\"udp\",\"peer\":\"127.0.0.1:5\"}\n",
        );
        for e in tx.poll(10) {
            if let LinkEvent::Transmit(d) = e {
                let h: String = d.iter().map(|b| format!("{b:02x}")).collect();
                for t in [0.01, 0.02] {
                    lines.push_str(&format!(
                        "{{\"ev\":\"dgram\",\"conn\":3,\"proto\":\"udp\",\"dir\":\"c2s\",\"t\":{t},\"hex\":\"{h}\"}}\n"
                    ));
                }
            }
        }
        let d = decode(lines.as_bytes());
        let got: Vec<_> = d.of(3, true).collect();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].shell(), Some(0x7f));
        assert_eq!(d.conn_by_port(5), Some(3));
    }
}
