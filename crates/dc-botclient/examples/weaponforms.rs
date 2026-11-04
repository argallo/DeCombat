//! Decodes the weapon, damage and kill messages of one capture connection with the crate's readers and prints one
//! JSON object per message (the shooter task's analysis tool; captures are read, never copied into the tree).
//!
//! ```text
//! cargo run -p dc-botclient --example weaponforms -- <capture.jsonl> (--conn <n> | --port <client port>)
//!       [--updates] [--from <t>] [--to <t>]
//! ```
//! `--updates` also prints every unreliable 0x64 (flag word, control word, aim rotation, position).

use dc_botclient::capdecode::{decode_file, unbundle};
use dc_botclient::record::hex;
use dc_botclient::weapon::{
    read_damage, read_fire, read_kill, read_status, read_update_head, read_weapon_change,
    read_weapon_grant,
};
use dc_wire::udp::BitWriter;
use serde_json::{json, Value};

fn num(s: &str) -> Option<f64> {
    s.strip_prefix("0x").map_or_else(
        || s.parse().ok(),
        |h| u64::from_str_radix(h, 16).ok().map(|v| v as f64),
    )
}

fn body(bits: &BitWriter) -> BitWriter {
    let mut r = bits.reader();
    r.read_u8();
    r.read_u8();
    r.read_bit_string(r.remaining_bits())
}

fn main() {
    let mut a = std::env::args().skip(1);
    let path = a.next().expect("capture path");
    let (mut conn, mut port, mut updates, mut from, mut to) = (None, None, false, 0.0, f64::MAX);
    while let Some(x) = a.next() {
        match x.as_str() {
            "--conn" => conn = a.next().and_then(|v| num(&v)).map(|v| v as u32),
            "--port" => port = a.next().and_then(|v| num(&v)).map(|v| v as u16),
            "--updates" => updates = true,
            "--from" => from = a.next().and_then(|v| num(&v)).unwrap_or(0.0),
            "--to" => to = a.next().and_then(|v| num(&v)).unwrap_or(f64::MAX),
            o => panic!("unknown option {o}"),
        }
    }
    let d = decode_file(std::path::Path::new(&path)).expect("read capture");
    if let Some(p) = port {
        conn = d.conn_by_port(p);
    }
    let conn = conn.expect("--conn or --port");
    for m in d
        .msgs
        .iter()
        .filter(|m| m.conn == conn && m.t >= from && m.t <= to)
    {
        let parts = if m.c2s {
            vec![m.bits.clone()]
        } else {
            unbundle(&m.bits)
        };
        for bits in parts {
            let b = bits.as_bytes();
            let sm = if m.c2s { 11 } else { 13 };
            if b.first() != Some(&sm) || b.len() < 2 {
                continue;
            }
            let sh = b[1];
            let inner = body(&bits);
            let mut r = inner.reader();
            let v: Option<Value> = match (m.c2s, sh) {
                (true, 0x82) => read_fire(&mut r).map(|f| {
                    json!({"wkind": f.wkind, "weapon_id": f.weapon_id, "ammo_id": f.ammo_id,
                        "weapon_pos": f.weapon_pos, "origin": f.origin, "aim": f.aim, "yaw": f.yaw, "dir": f.dir,
                        "seed": f.seed, "moving": f.moving, "wflag": f.wflag, "mode1": f.mode1, "frac": f.frac,
                        "shot_no": f.shot_no, "time_ms": f.time_ms,
                        "extra": f.extra.map(|x| json!({"obj": x.obj, "kind": x.kind, "a": x.a, "b": x.b})),
                        "flag_c": f.flag_c, "last": f.last, "hit": f.hit, "stat": f.stat,
                        "left_bits": r.remaining_bits()})
                }),
                (true, 0x7f) => {
                    let c = read_weapon_change(&mut r);
                    Some(json!({"slot": c.slot, "weapon_id": c.weapon_id, "aux": c.aux}))
                }
                (true, 0x76) | (true, 0x83) | (true, 0x80) | (true, 0x81) | (true, 0x4d) | (true, 0xca)
                | (true, 0x89) => Some(json!({"body": hex(inner.as_bytes()), "bits": inner.bit_len()})),
                (true, 0x64) if updates || !m.reliable => {
                    if !updates {
                        None
                    } else {
                        read_update_head(&mut r).map(|u| {
                            json!({"flags": format!("0x{:04x}", u.flags), "control": format!("0x{:x}", u.control),
                                   "aim_q": u.aim_q, "weapon_f": u.weapon_f, "yaw8c": u.yaw8c,
                                   "angles": u.body_angles, "pos": u.pos, "seq": u.move_seq, "complete": u.complete})
                        })
                    }
                }
                (false, 0xb2) | (false, 0xb3) => {
                    let g = read_weapon_grant(&mut r, sh == 0xb3);
                    Some(json!({"weapon": g.weapon, "ammo_kind": g.ammo_kind, "ammo": g.ammo, "clip": g.clip,
                                "class_pos": g.class_pos, "slot": g.slot}))
                }
                (false, 0x74) => {
                    let x = read_damage(&mut r);
                    Some(json!({"from_dir": x.from_dir, "mode": x.mode, "ammo_kind": x.ammo_kind,
                                "dmg_kind": x.dmg_kind, "zone": x.zone, "part": x.part, "damage": x.damage}))
                }
                (false, 0x66) => Some(json!({"status": format!("{:?}", read_status(&mut r))})),
                (false, 0x6d) => {
                    let k = read_kill(&mut r);
                    Some(json!({"kill": format!("{k:?}"), "left_bits": r.remaining_bits()}))
                }
                (false, 0x67) | (false, 0x6a) | (false, 0xa0) | (false, 0x6b) | (false, 0x6c) | (false, 0x4a)
                | (false, 0x46) | (false, 0x41) | (false, 0x48) | (false, 0x3f) | (false, 0x65) | (false, 0xb5)
                | (false, 0x44) | (false, 0x45) | (false, 0x99) | (false, 0x7f) | (false, 0xc8) | (false, 0xd7)
                | (false, 0xdb) | (false, 0x7d) => Some(json!({"head": hex(&b[..b.len().min(24)]), "bits": bits.bit_len()})),
                _ => None,
            };
            if let Some(v) = v {
                println!(
                    "{}",
                    json!({"t": m.t, "dir": if m.c2s {"c2s"} else {"s2c"}, "rel": m.reliable,
                           "shell": format!("0x{sh:02x}"), "head": hex(&b[..b.len().min(24)]), "f": v})
                );
            }
        }
    }
}
