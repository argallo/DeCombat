//! Prints the session messages of a captap capture (format 1), one JSON object per line.
//!
//! ```text
//! cargo run -p dc-botclient --example capdecode -- <capture.jsonl> [--conn <n> | --port <client port>]
//!       [--dir c2s|s2c|both] [--shell <id>[,<id>...]] [--full] [--conns]
//! ```
//! `--shell` filters on the shell id (hex with 0x or decimal); `--full` prints the whole message as hex (else the
//! first 48 bytes); `--conns` lists the UDP connections and their client addresses. Captures are read, never
//! copied into the tree (REFERENCE_TESTING.md §5).

use dc_botclient::capdecode::{decode_file, unbundle, CapMsg};
use dc_botclient::record::hex;
use serde_json::json;

fn num(s: &str) -> Option<u32> {
    s.strip_prefix("0x")
        .map_or_else(|| s.parse().ok(), |h| u32::from_str_radix(h, 16).ok())
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: capdecode <capture.jsonl> [--conn n|--port p] [--dir c2s|s2c|both] [--shell ids] [--full] [--conns]");
        std::process::exit(2);
    };
    let (mut conn, mut port, mut dir, mut shells, mut full, mut list) = (
        None,
        None,
        "both".to_string(),
        Vec::<u8>::new(),
        false,
        false,
    );
    while let Some(a) = args.next() {
        match a.as_str() {
            "--conn" => conn = args.next().and_then(|v| num(&v)),
            "--port" => port = args.next().and_then(|v| num(&v)),
            "--dir" => dir = args.next().unwrap_or_default(),
            "--shell" => {
                shells = args
                    .next()
                    .unwrap_or_default()
                    .split(',')
                    .filter_map(|s| num(s.trim()).map(|v| v as u8))
                    .collect()
            }
            "--full" => full = true,
            "--conns" => list = true,
            other => {
                eprintln!("unknown option {other}");
                std::process::exit(2);
            }
        }
    }
    let d = match decode_file(std::path::Path::new(&path)) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{path}: {e}");
            std::process::exit(1);
        }
    };
    if list {
        for c in d.conns.values() {
            println!(
                "{}",
                json!({"conn": c.conn, "peer": c.peer, "c2s": c.c2s_datagrams, "s2c": c.s2c_datagrams, "first_t": c.first_t})
            );
        }
        return;
    }
    if let Some(p) = port {
        conn = d.conn_by_port(p as u16);
    }
    let show = |m: &CapMsg, bits: &dc_wire::udp::BitWriter| {
        let b = bits.as_bytes();
        let sm = if m.c2s { 11 } else { 13 };
        let sh = (b.first() == Some(&sm)).then(|| b.get(1).copied().unwrap_or(0));
        if !shells.is_empty() && !sh.is_some_and(|s| shells.contains(&s)) {
            return;
        }
        let h = if full {
            hex(b)
        } else {
            hex(&b[..b.len().min(48)])
        };
        println!(
            "{}",
            json!({"t": m.t, "seq": m.seq, "conn": m.conn, "dir": if m.c2s {"c2s"} else {"s2c"}, "rel": m.reliable,
                   "id": b.first(), "shell": sh.map(|s| format!("0x{s:02x}")), "bits": bits.bit_len(), "hex": h})
        );
    };
    for m in &d.msgs {
        if conn.is_some_and(|c| c != m.conn) {
            continue;
        }
        if (dir == "c2s" && !m.c2s) || (dir == "s2c" && m.c2s) {
            continue;
        }
        if m.c2s {
            show(m, &m.bits);
        } else {
            for inner in unbundle(&m.bits) {
                show(m, &inner);
            }
        }
    }
}
