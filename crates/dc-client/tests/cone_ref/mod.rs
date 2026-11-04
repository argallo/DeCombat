//! The reference replay of the 2009 shot cone, as `crates/dc-dserver/tests/fire_cone_cap.rs` does it (the code that
//! replays 3807 of 3807 real-client pellets): the table rebuilt by its generator, the aim's axes in f64, the integer
//! half-width implied by the sent direction, the row interval for the sent `frac`. The replay, the generator and the
//! row interval are the shared `dc-cone` crate's (its inverse, [`dc_cone::replay`], in f64 axes); this module adds
//! the capture readers.
// SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-1.3 (generator), AC-2 (cone), AC-5 (replay method)
// DISASM: cshell+0x176cb0 (cone), cshell+0x2973a0 (table read)
#![allow(dead_code)]

use dc_botclient::capdecode::{unbundle, Decoded};
use dc_botclient::shell::s2c;
use dc_botclient::weapon::{read_fire, FireReport};

/// A capture folder's event file when present (under `DC_CAPTURES`).
pub fn capture_named(folder: &str) -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("DC_CAPTURES") {
        let c = std::path::PathBuf::from(p)
            .join(folder)
            .join("capture.jsonl");
        return c.is_file().then_some(c);
    }
    None
}

/// The table: `rand()` from state 1, times `f32(1 / 32766)`, six significant digits, parsed back.
// SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-1.3
pub fn rebuilt_table() -> Vec<f32> {
    dc_cone::generated_table()
}

/// Angle between two directions (radians).
pub fn angle(a: [f32; 3], b: [f32; 3]) -> f64 {
    dc_cone::angle(a, b)
}

/// `Some(h)` when the sent direction is the aim's forward axis tilted by the two table draws of `stat` with one
/// integer half-width `h`, within single-precision noise (2e-5 rad).
// SPEC: WEAPON_FIRE_2009_addendum_cone.md AC-5
pub fn replay(t: &[f32], f: &FireReport) -> Option<i64> {
    dc_cone::replay(t, f.aim, f.dir, f.stat)
}

/// The cone keys of a weapon's row (`None` when the row lacks `MinPerturb` or `MaxPerturb`).
pub fn row_of(doc: &dc_bute::ButeDoc, weapon: u16) -> Option<dc_cone::ConeRow> {
    let sec = doc.get(&format!("Weapon{weapon}"))?;
    dc_cone::num(sec, dc_cone::key::MIN)?;
    dc_cone::num(sec, dc_cone::key::MAX)?;
    Some(dc_cone::ConeRow::of_section(sec))
}

/// The half-width for spread `f` (single precision); 0 when the row's gate refuses (no draw).
// DISASM: cshell+0x176ce1..0x176dc6
pub fn half_width(
    row: &dc_cone::ConeRow,
    zoom: bool,
    triple: bool,
    scale: f32,
    f: f32,
) -> Option<i64> {
    Some(dc_cone::half_width(row, zoom, triple, scale, f).unwrap_or(0))
}

/// The UP_FIRE reports of a capture, per connection, in order: (capture time, report).
pub fn fires(d: &Decoded) -> std::collections::BTreeMap<u32, Vec<(f64, FireReport)>> {
    let mut out: std::collections::BTreeMap<u32, Vec<(f64, FireReport)>> = Default::default();
    for m in d.msgs.iter().filter(|m| m.c2s && m.shell() == Some(0x82)) {
        let mut r = m.bits.reader();
        r.read_u8();
        r.read_u8();
        if let Some(f) = read_fire(&mut r) {
            out.entry(m.conn).or_default().push((m.t, f));
        }
    }
    out
}

/// Server shell messages of a connection up to `t` (bundles split): (shell id, whole message).
pub fn shells_before(d: &Decoded, conn: u32, t: f64) -> Vec<(u8, dc_wire::udp::BitWriter)> {
    let mut v = Vec::new();
    for m in d.of(conn, false).filter(|m| m.t <= t) {
        for part in unbundle(&m.bits) {
            let b = part.as_bytes();
            if b.len() > 2 && b[0] == s2c::SHELL_MSG {
                v.push((b[1], part.clone()));
            }
        }
    }
    v
}
