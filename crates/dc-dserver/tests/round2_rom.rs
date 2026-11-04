//! ROM- and capture-gated tests of the round-2 rules against the reference stack's forms on the real client's link (captures
//! tdm6 and tdm4, connection 6; REFERENCE forms the real 2009 client accepted). Skipped without the ROM or the captures.
//! Captures are read in place and never copied (REFERENCE_TESTING.md §5).

use dc_dserver::world::{load_from_rom, NameForm};
use dc_wire::udp::BitWriter;

/// A capture folder under `DC_CAPTURES` when present.
fn capture_named(folder: &str) -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("DC_CAPTURES") {
        let c = std::path::PathBuf::from(p)
            .join(folder)
            .join("capture.jsonl");
        return c.is_file().then_some(c);
    }
    None
}

/// The server's messages on one link of a capture, reassembled by our own transport reader, with the tap time.
fn link_messages(cap: &std::path::Path, conn: i64) -> Vec<(f64, BitWriter)> {
    use dc_net_engine::{Link, LinkConfig, LinkEvent};
    let unhex = |s: &str| -> Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
            .collect()
    };
    let text = std::fs::read_to_string(cap).unwrap();
    let mut link: Option<Link> = None;
    let mut msgs = Vec::new();
    for line in text.lines() {
        let Ok(j) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if j["ev"] != "dgram" || j["dir"] != "s2c" || j["conn"].as_i64() != Some(conn) {
            continue;
        }
        let t = j["t"].as_f64().unwrap();
        let ms = (t * 1000.0) as u32;
        let l = link.get_or_insert_with(|| Link::new(ms, LinkConfig::default()));
        l.handle_datagram(ms, &unhex(j["hex"].as_str().unwrap()));
        for ev in l.poll(ms) {
            if let LinkEvent::Message { bits, .. } = ev {
                for m in dc_botclient::capdecode::unbundle(&bits) {
                    msgs.push((t, m));
                }
            }
        }
    }
    msgs
}

/// The world file the capture's first file list announces.
fn capture_world(msgs: &[(f64, BitWriter)]) -> Option<String> {
    for (_, m) in msgs {
        let mut r = m.reader();
        if r.read_u8() == 0x32 && r.remaining_bits() >= 48 {
            r.read_u16();
            r.read_u32();
            return Some(String::from_utf8_lossy(&r.read_zstr8()).into_owned());
        }
    }
    None
}

const CAPTURES: [(&str, &str); 2] = [
    ("tdm6", "20261025_065016_tdm6"),
    ("tdm4", "20261025_002236_tdm4"),
];

/// DSV-47 (P3 / P4): our world variables of the tdm6 and tdm4 worlds are the reference stack's world-settings variables,
/// name for name and value for value (the reference stack's `S2C_EVAR`s up to its `S2C_LOAD_WORLD`), and we send no other
/// variable: the reference stack's variables outside the 33 rows are server configuration and are not ours.
#[test]
fn rom_world_vars_match_the_reference() {
    use dc_dserver::evars::{evar_messages, WORLD_VARS};
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let mut checked = 0;
    for (label, folder) in CAPTURES {
        let Some(cap) = capture_named(folder) else {
            println!("skipped: no {label} capture");
            continue;
        };
        let msgs = link_messages(&cap, 6);
        let world = capture_world(&msgs).expect("a file list");
        let mut theirs: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        for (_, m) in &msgs {
            let mut r = m.reader();
            match r.read_u8() {
                15 => {
                    let n = r.read_zstr8();
                    let v = r.read_zstr8();
                    theirs.retain(|x| x.0 != n);
                    theirs.push((n, v));
                }
                dc_dserver::msg::S2C_LOAD_WORLD => break,
                _ => {}
            }
        }
        let w = load_from_rom(&rom, &world, NameForm::Listed, None).expect("the capture's world");
        assert_eq!(
            w.world_vars.len(),
            33,
            "{label}: every row has its property"
        );
        let ours: Vec<(Vec<u8>, Vec<u8>)> = evar_messages(&w.world_vars, &[])
            .iter()
            .map(|m| {
                let mut r = m.reader();
                assert_eq!(r.read_u8(), 15);
                (r.read_zstr8(), r.read_zstr8())
            })
            .collect();
        assert_eq!(
            ours.len(),
            33,
            "{label}: every client name found in the ROM"
        );
        let key = |n: &[u8]| (dc_world::hash16(n), n.len());
        let in_rows = |n: &[u8]| WORLD_VARS.iter().any(|r| r.client == key(n));
        let mut theirs_ws: Vec<(Vec<u8>, Vec<u8>)> =
            theirs.iter().filter(|x| in_rows(&x.0)).cloned().collect();
        let mut ours_sorted = ours.clone();
        theirs_ws.sort();
        ours_sorted.sort();
        assert_eq!(ours_sorted, theirs_ws, "{label}: names and values");
        let others = theirs.len() - theirs_ws.len();
        println!(
            "{label} world {}: 33 of 33 world variables equal (names and values); the reference stack's {others} other \
             variables are not sent",
            &w.sha256[..16]
        );
        checked += 1;
    }
    if checked == 0 {
        println!("skipped: no capture");
    }
}

/// The reference stack's teleports of the spawn sets on a link (shell 0xa0 with the teleport flag 0x0020): position and box.
fn reference_teleports(msgs: &[(f64, BitWriter)]) -> Vec<([f32; 3], [f32; 3])> {
    let mut out = Vec::new();
    for (_, m) in msgs {
        let mut r = m.reader();
        if r.read_u8() != 13 || r.remaining_bits() < 8 || r.read_u8() != 0xa0 {
            continue;
        }
        // `[flags:u16 = 0x0020][move_seq:u8][pos:vec96][box:vec96]` (MOVEMENT_2009.md §12.4)
        if r.remaining_bits() < 16 + 8 + 192 || r.read_u16() != 0x0020 {
            continue;
        }
        r.read_u8();
        let p = [r.read_f32(), r.read_f32(), r.read_f32()];
        let d = [r.read_f32(), r.read_f32(), r.read_f32()];
        out.push((p, d));
    }
    out
}

/// DSV-48 (P8): a spawn rests on the floor below the spawn point, found by our own collision model. On the tdm6 world
/// (the rust3 world, where the real client fell 36 units after each of our spawns) every reference spawn teleport of the
/// capture that lies on a spawn point's column is compared with our floor placement for that point and box: within
/// one unit of the reference stack's height, never above the raw point; `-rawspawn` keeps the raw point.
#[test]
fn rom_spawns_rest_on_the_floor() {
    use dc_dserver::game::{Game, GameTuning};
    use dc_dserver::rom_tables::RomTables;
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let Some(cap) = capture_named(CAPTURES[0].1) else {
        println!("skipped: no tdm6 capture");
        return;
    };
    let mut teleports = Vec::new();
    for conn in [5, 6] {
        teleports.extend(reference_teleports(&link_messages(&cap, conn)));
    }
    let msgs = link_messages(&cap, 6);
    let world = capture_world(&msgs).expect("a file list");
    let w = load_from_rom(&rom, &world, NameForm::Listed, None).expect("the capture's world");
    let g = Game::new(GameTuning::default(), &w, RomTables::default());
    let raw = GameTuning {
        spawn_on_floor: false,
        ..GameTuning::default()
    };
    let g_raw = Game::new(raw, &w, RomTables::default());
    let mut compared = 0;
    let mut worst = 0f32;
    let mut drop = 0f32;
    for (p, d) in &teleports {
        let Some(s) = w
            .spawns
            .iter()
            .find(|s| (s.pos[0] - p[0]).abs() < 0.5 && (s.pos[2] - p[2]).abs() < 0.5)
        else {
            continue;
        };
        let ours = g.floor_point(s.pos, *d);
        assert_eq!(
            g_raw.floor_point(s.pos, *d),
            s.pos,
            "-rawspawn keeps the point"
        );
        assert!(
            ours[1] < s.pos[1] + 0.01,
            "never above the point: {ours:?} {:?}",
            s.pos
        );
        let dy = (ours[1] - p[1]).abs();
        worst = worst.max(dy);
        drop = drop.max(s.pos[1] - ours[1]);
        assert!(dy < 1.0, "spawn {:?}: ours {ours:?}, reference {p:?}", s.pos);
        compared += 1;
    }
    assert!(
        compared >= 4,
        "{compared} reference teleports on spawn points ({})",
        teleports.len()
    );
    println!(
        "tdm6 world: {compared} reference spawn teleports on spawn points ({} in all), our floor height within \
         {worst:.3} units of every one (largest drop below the raw point {drop:.1})",
        teleports.len()
    );
}

/// The reference stack's `GS_MATCH_BEGIN` on the real client's link.
fn reference_match_begin(msgs: &[(f64, BitWriter)]) -> Option<BitWriter> {
    msgs.iter()
        .find(|(_, m)| m.as_bytes().len() > 2 && m.as_bytes()[0] == 13 && m.as_bytes()[1] == 0x45)
        .map(|(_, m)| m.clone())
}

fn read_vec(r: &mut dc_wire::udp::BitReader<'_>) -> [f32; 3] {
    [r.read_f32(), r.read_f32(), r.read_f32()]
}

/// DSV-50 (P7): the intro from the room line our lobby writes for the capture's map (the TDM row of the ROM mode
/// data) and the world's spawn points carries the reference stack's strings, corners, zoom, rotation, sizes and team bases: the
/// whole 0x45 (with the reference stack's `waiting_banner` 0) equals the reference stack's message bit for bit.
#[test]
fn rom_match_intro_matches_the_reference() {
    use dc_dserver::game::{Game, GameTuning};
    use dc_dserver::gs;
    use dc_dserver::rom_tables::RomTables;
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let mut checked = 0;
    for (label, folder) in CAPTURES {
        let Some(cap) = capture_named(folder) else {
            println!("skipped: no {label} capture");
            continue;
        };
        let msgs = link_messages(&cap, 6);
        let world = capture_world(&msgs).expect("a file list");
        let theirs = reference_match_begin(&msgs).expect("the reference stack's 0x45");
        let line = tdm_room_line(&rom, &world).expect("a TDM room line for the map");
        let w = load_from_rom(&rom, &world, NameForm::Listed, None).expect("the capture's world");
        let (tables, _) = RomTables::load(&rom);
        let mut g = Game::new(GameTuning::default(), &w, tables);
        let dc_dserver::pipe::Control::Room(room) = dc_dserver::pipe::parse_control(&line) else {
            panic!("room line");
        };
        g.set_room(&room);
        let intro = g.match_intro();
        // the reference stack's intro block (mode class 1: no caption, team bases read)
        let mut r = theirs.reader();
        r.read_u8();
        r.read_u8();
        let round_secs = r.read_u32();
        assert!(r.read_bit(), "{label}: reference intro");
        let first = r.read_bit();
        let countdown = r.read_u32();
        let head = r.read_zstr8();
        let body = r.read_zstr8();
        let map_a = r.read_zstr8();
        let (mmin, mmax) = (read_vec(&mut r), read_vec(&mut r));
        let radar = r.read_zstr8();
        let map_b = r.read_zstr8();
        let (rmin, rmax) = (read_vec(&mut r), read_vec(&mut r));
        let (zoom, rot, p3, p4) = (r.read_f32(), r.read_f32(), r.read_u32(), r.read_u32());
        let (b1, b2) = (read_vec(&mut r), read_vec(&mut r));
        let round_no = r.read_u32();
        assert_eq!(intro.notice_head, head, "{label}: caption");
        assert_eq!(intro.notice_body, body, "{label}: text");
        assert_eq!(intro.map_image_a, map_a, "{label}: large map picture");
        assert_eq!(intro.map_image_b, map_b, "{label}: second picture");
        assert_eq!(intro.radar_image, radar, "{label}: radar picture");
        assert_eq!(
            (intro.map_min, intro.map_max),
            (mmin, mmax),
            "{label}: map corners"
        );
        assert_eq!(
            (intro.radar_min, intro.radar_max),
            (rmin, rmax),
            "{label}: radar corners"
        );
        assert_eq!(
            (
                intro.radar_zoom,
                intro.radar_rot,
                intro.radar_p3,
                intro.radar_p4
            ),
            (zoom, rot, p3, p4),
            "{label}: zoom, rotation, sizes"
        );
        let base_diff: f32 = (0..3)
            .map(|i| {
                (intro.base_team1[i] - b1[i])
                    .abs()
                    .max((intro.base_team2[i] - b2[i]).abs())
            })
            .fold(0f32, f32::max);
        assert!(
            base_diff < 0.01,
            "{label}: team bases {:?} {:?} vs {b1:?} {b2:?}",
            intro.base_team1,
            intro.base_team2
        );
        // the bases bit for bit (f32 sum in the world's order times the f32 reciprocal of the count: tdm6 and tdm4)
        assert_eq!(
            (intro.base_team1, intro.base_team2),
            (b1, b2),
            "{label}: team bases bit for bit"
        );
        let ours = gs::match_begin(
            &gs::MatchBegin {
                round_secs,
                waiting_banner: first,
                countdown_secs: countdown,
                intro: Some(intro),
                round_no,
            },
            1,
        );
        assert_eq!(ours.bit_len(), theirs.bit_len(), "{label}: 0x45 length");
        assert_eq!(
            ours.as_bytes(),
            theirs.as_bytes(),
            "{label}: 0x45 bit for bit"
        );
        println!(
            "{label}: 0x45 intro equal to the reference stack's bit for bit ({} bits), team bases included",
            theirs.bit_len()
        );
        checked += 1;
    }
    if checked == 0 {
        println!("skipped: no capture");
    }
}

/// The room line our lobby writes (dc-bute's room line) for a TDM room on the capture's map.
fn tdm_room_line(rom: &std::path::Path, world: &str) -> Option<String> {
    let vfs = dc_rez::RezVfs::new(rom);
    let gr = dc_bute::GameRoomData::from_vfs(&vfs).ok()?;
    let unq = |s: &str| s.trim().trim_matches('"').to_string();
    let mode = gr.modes(false).iter().position(|m| {
        m.get("Key")
            .is_some_and(|k| unq(k).eq_ignore_ascii_case("TDM"))
    })?;
    let base = |p: &str| {
        let n = p.replace('/', "\\").to_ascii_uppercase();
        let b = n.rsplit('\\').next().unwrap_or(&n).to_string();
        b.strip_suffix(".DAT").map(str::to_string).unwrap_or(b)
    };
    let want = base(world);
    for (i, m) in gr.maps().iter().enumerate() {
        if !m.contains_key("GMS_TDM") {
            continue;
        }
        let (_md, doc) = dc_bute::gms_file_with_vfs(&vfs, &gr, i, mode, false);
        if doc
            .get_str("MapInfo", "Map")
            .is_some_and(|w| base(&unq(w)) == want)
        {
            let cfg = dc_bute::RoomConfig {
                map: i,
                mode,
                ..dc_bute::RoomConfig::default()
            };
            return Some(dc_bute::room_line_with_vfs(
                &vfs,
                &gr,
                &cfg,
                &dc_bute::RoomOptions::default(),
            ));
        }
    }
    None
}

/// A kind-2 create record as far as the door comparison needs it: name, blob bytes and length, flag words, raw
/// position.
#[derive(Debug, Clone, PartialEq)]
struct BrushCreate {
    name: Vec<u8>,
    blob: Option<(usize, Vec<u8>)>,
    flags: (u16, u16, u32, u32),
    pos: [f32; 3],
}

fn brush_create(rec: &BitWriter) -> Option<BrushCreate> {
    let mut q = rec.reader();
    let lo = q.read_u8();
    if lo == 0 {
        return None;
    }
    let cf = if lo & 0x80 != 0 {
        lo as u16 | (q.read_u8() as u16) << 8
    } else {
        lo as u16
    };
    q.read_u16();
    if cf & 1 == 0 || cf & 0x0008 == 0 {
        return None;
    }
    let kb = q.read_u8();
    if kb & 0x3f != 2 {
        return None;
    }
    q.skip_bits(96);
    let blob = (kb & 0x40 != 0).then(|| {
        let n = if kb & 0x80 != 0 {
            q.read_u16() as usize
        } else {
            q.read_u8() as usize
        };
        let b = q.read_bit_string(n);
        (n, b.as_bytes().to_vec())
    });
    let name = q.read_zstr8();
    let flags = (q.read_u16(), q.read_u16(), q.read_u32(), q.read_u32());
    if flags.0 & 0x0100 == 0 {
        return None;
    }
    let pos = read_vec(&mut q);
    Some(BrushCreate {
        name,
        blob,
        flags,
        pos,
    })
}

/// DSV-46 (doors slice 1): the tdm4 world's sliding doors as we create them equal the reference stack's door creates on the
/// real client's link: the same names, the 25-bit type-0x39 blob bit for bit, the flag words (the user word without
/// its high surface byte, which we do not derive) and the full-precision position (the model origin) bit for bit.
/// The capture has no door movement record (no door ever moved in it), so the moving form is not compared.
#[test]
fn rom_doors_match_the_reference_creates() {
    use dc_dserver::objects::PosBox;
    use dc_dserver::world::BrushClass;
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let Some(cap) = capture_named(CAPTURES[1].1) else {
        println!("skipped: no tdm4 capture");
        return;
    };
    let msgs = link_messages(&cap, 6);
    let world = capture_world(&msgs).expect("a file list");
    let mut theirs = Vec::new();
    let mut door_changes = 0;
    let mut door_ids = Vec::new();
    for (_, m) in &msgs {
        let mut r = m.reader();
        if r.read_u8() != 8 {
            continue;
        }
        while r.remaining_bits() >= 32 {
            let len = r.read_u32() as usize;
            let rec = r.read_bit_string(len);
            if let Some(c) = brush_create(&rec) {
                if c.blob.as_ref().is_some_and(|b| b.1.first() == Some(&0x39)) {
                    let mut q = rec.reader();
                    q.read_u8();
                    door_ids.push(q.read_u16());
                    theirs.push(c);
                }
                continue;
            }
            // a change record of a door object
            let mut q = rec.reader();
            let lo = q.read_u8();
            if lo != 0 {
                if lo & 0x80 != 0 {
                    q.read_u8();
                }
                if door_ids.contains(&q.read_u16()) {
                    door_changes += 1;
                }
            }
        }
    }
    assert_eq!(theirs.len(), 3, "the reference stack's three doors");
    let w = load_from_rom(&rom, &world, NameForm::Listed, None).expect("the capture's world");
    let pbox = PosBox {
        min: w.extent_min,
        max: w.extent_max,
    };
    let mut ours: Vec<BrushCreate> = w
        .doors
        .iter()
        .enumerate()
        .filter_map(|(i, d)| {
            let b = w
                .brushes
                .iter()
                .find(|b| b.class == BrushClass::Door && b.name == d.name)?;
            assert_eq!(b.pos, d.closed, "closed position = the model origin");
            brush_create(&dc_dserver::worldobj::brush_record(
                1000 + i as u16,
                b,
                &pbox,
            ))
        })
        .collect();
    ours.sort_by(|a, b| a.name.cmp(&b.name));
    theirs.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(ours.len(), theirs.len(), "the same doors");
    for (o, t) in ours.iter().zip(&theirs) {
        assert_eq!(o.name, t.name);
        assert_eq!(o.blob, t.blob, "door blob");
        let mask = |f: (u16, u16, u32, u32)| (f.0, f.1, f.2 & 0x00ff_ffff, f.3);
        assert_eq!(mask(o.flags), mask(t.flags), "flag words");
        assert_eq!(
            o.pos.map(f32::to_bits),
            t.pos.map(f32::to_bits),
            "position bit for bit"
        );
    }
    println!(
        "tdm4 world: 3 of 3 sliding doors equal to the reference stack's creates (blob, flag words without the surface byte, \
         position); the reference stack's door change records in the capture: {door_changes}"
    );
}

/// DSV-52 (P2): the kill volumes the session acts on are the kill volumes the world objects create on the clients
/// (the planned set of a TDM room), each closed volume contains its own box centre, and no TDM spawn placement of
/// the world lies inside one (a player must not die at its spawn). tdm6 and tdm4 worlds.
#[test]
fn rom_kill_volumes_act_and_spare_the_spawns() {
    use dc_dserver::game::{Game, GameTuning};
    use dc_dserver::rom_tables::RomTables;
    use dc_dserver::worldobj::{plan_scene, WorldObject, WorldObjects, ZoneClass};
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let (tables, _) = RomTables::load(&rom);
    for (label, folder) in CAPTURES {
        let Some(cap) = capture_named(folder) else {
            println!("skipped: no {label} capture");
            continue;
        };
        let world = capture_world(&link_messages(&cap, 6)).expect("a file list");
        let w = load_from_rom(&rom, &world, NameForm::Listed, None).expect("the capture's world");
        let line = tdm_room_line(&rom, &world).expect("room line");
        let dc_dserver::pipe::Control::Room(room) = dc_dserver::pipe::parse_control(&line) else {
            panic!("room line");
        };
        let mut next = 1000u16..;
        let objs = plan_scene(
            &w,
            &tables,
            WorldObjects::World,
            ZoneClass::Room,
            1,
            room.get("trig"),
            || next.next().unwrap(),
        );
        let created: Vec<&[u8]> = objs
            .iter()
            .filter_map(|o| match o {
                WorldObject::Volume { volume, .. }
                    if volume.kind == dc_dserver::world::VolumeKind::KillVolume =>
                {
                    Some(volume.name.as_slice())
                }
                _ => None,
            })
            .collect();
        let active: Vec<&dc_dserver::killvol::VolumeShape> = w
            .kill_shapes
            .iter()
            .filter(|s| created.iter().any(|n| n.eq_ignore_ascii_case(&s.name)))
            .collect();
        assert_eq!(
            active.len(),
            created.len(),
            "{label}: a shape per created kill volume"
        );
        for s in &active {
            // one unit inside and outside the middle of each polygon (the polygon normals point out of the volume)
            for f in &s.faces {
                let n = f.corners.len() as f32;
                let m = [0, 1, 2].map(|k| f.corners.iter().map(|c| c[k]).sum::<f32>() / n);
                let inside = [0, 1, 2].map(|k| m[k] - f.normal[k]);
                let outside = [0, 1, 2].map(|k| m[k] + f.normal[k]);
                assert!(
                    s.contains(inside),
                    "{label}: inside a kill volume at {inside:?}"
                );
                assert!(
                    !s.contains(outside),
                    "{label}: outside a kill volume at {outside:?}"
                );
            }
        }
        let mut g = Game::new(GameTuning::default(), &w, tables.clone());
        g.set_room(&room);
        let dims = GameTuning::default().player_dims;
        let mut spawns = 0;
        for (sp, mode) in w.spawns.iter().zip(&w.spawn_modes) {
            // the TDM spawn points (mode filter initials, as the intro's team bases)
            let initials: Vec<u8> = mode
                .as_deref()
                .unwrap_or_default()
                .iter()
                .copied()
                .filter(u8::is_ascii_uppercase)
                .collect();
            if initials != b"TDM" {
                continue;
            }
            let p = g.floor_point(sp.pos, dims);
            assert!(
                !active.iter().any(|s| s.contains(p)),
                "{label}: spawn {p:?} inside a kill volume"
            );
            spawns += 1;
        }
        let pending = active.iter().filter(|s| s.pending_ms > 0).count();
        println!(
            "{label}: {} kill volumes act ({pending} with a pending time), of {} in the file; {spawns} TDM spawn \
             placements outside all of them",
            active.len(),
            w.kill_shapes.len()
        );
    }
}

/// DSV-60: our whole variable list before `S2C_LOAD_WORLD` (the 33 world-settings rows, the step height and the
/// world-north angle, default configuration) equals the reference stack's list on the tdm6 and tdm4 worlds restricted to the
/// variables the 2009 client reads: names, value texts and, on the link that loaded first (conn 5), the order. The
/// reference stack's other variables (51 of its 86 before the world load) have no reader in the client and are not sent.
// OBSERVED: capture 20261025_065016_tdm6, 20261025_002236_tdm4 (REFERENCE)
// EVIDENCE: engine+0xd760 (the client's only server-variable read); DISASM: the slot +0x134 call sites listed in
// `clientvars.rs`
#[test]
fn rom_preload_vars_match_the_reference() {
    use dc_dserver::evars::{preload_messages, SEND_ORDER};
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let key = |n: &[u8]| (dc_world::hash16(n), n.len());
    let read = |m: &BitWriter| {
        let mut r = m.reader();
        assert_eq!(r.read_u8(), 15);
        (r.read_zstr8(), r.read_zstr8())
    };
    let mut checked = 0;
    for (label, folder) in CAPTURES {
        let Some(cap) = capture_named(folder) else {
            println!("skipped: no {label} capture");
            continue;
        };
        for conn in [5i64, 6] {
            let msgs = link_messages(&cap, conn);
            let world = capture_world(&msgs).expect("a file list");
            let mut theirs: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
            for (_, m) in &msgs {
                let mut r = m.reader();
                match r.read_u8() {
                    15 => {
                        let n = r.read_zstr8();
                        let v = r.read_zstr8();
                        theirs.retain(|x| x.0 != n);
                        theirs.push((n, v));
                    }
                    dc_dserver::msg::S2C_LOAD_WORLD => break,
                    _ => {}
                }
            }
            let w =
                load_from_rom(&rom, &world, NameForm::Listed, None).expect("the capture's world");
            let consts = dc_dserver::clientvars::vars(
                &w.client_vars,
                Some(dc_dserver::clientvars::DEFAULT_STEP_HEIGHT),
                Some(dc_dserver::clientvars::DEFAULT_WORLD_NORTH),
            );
            let ours: Vec<(Vec<u8>, Vec<u8>)> = preload_messages(&w.world_vars, &consts, &[])
                .iter()
                .map(read)
                .collect();
            assert_eq!(
                ours.len(),
                SEND_ORDER.len(),
                "{label}: every name found in the ROM"
            );
            let read_by_client: Vec<(Vec<u8>, Vec<u8>)> = theirs
                .iter()
                .filter(|x| SEND_ORDER.contains(&key(&x.0)))
                .cloned()
                .collect();
            assert_eq!(
                read_by_client.len(),
                SEND_ORDER.len(),
                "{label} conn {conn}: the reference stack sent every variable the client reads"
            );
            if conn == 5 {
                assert_eq!(
                    ours, read_by_client,
                    "{label} conn {conn}: names, values and order"
                );
            } else {
                let (mut a, mut b) = (ours.clone(), read_by_client.clone());
                a.sort();
                b.sort();
                assert_eq!(a, b, "{label} conn {conn}: names and values");
            }
            println!(
                "{label} conn {conn} world {}: {} of {} variables equal{}; {} reference variables without a client \
                 reader not sent",
                &w.sha256[..16],
                ours.len(),
                read_by_client.len(),
                if conn == 5 { " in order" } else { "" },
                theirs.len() - read_by_client.len()
            );
            checked += 1;
        }
    }
    if checked == 0 {
        println!("skipped: no capture");
    }
}

/// DSV-59: the spawn choice of a TDM room takes only the team's spawn points whose mode filter names the room's mode
/// (capital letters = the mode key, the rule of the intro's team bases). Both worlds (tdm6 JUNK_FLEA, tdm4
/// SNOWVALLEY) also hold team points of other modes, and every spawn teleport the reference stack sent on a spawn point's
/// column (both links) lies on a point of the room's mode.
// OBSERVED: capture 20261025_065016_tdm6, 20261025_002236_tdm4 (the reference stack's spawn teleports;
// REFERENCE, accepted by the real client); reports/dserver-round2.md findings (our TDM respawn on a
// point of another mode)
// DC-DEFAULT: DSV-59 (proposed)
#[test]
fn rom_spawn_choice_follows_the_mode_filter() {
    use dc_dserver::game::{Game, GameTuning};
    use dc_dserver::rom_tables::RomTables;
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let mut checked = 0;
    for (label, folder) in CAPTURES {
        let Some(cap) = capture_named(folder) else {
            println!("skipped: no {label} capture");
            continue;
        };
        let msgs = link_messages(&cap, 6);
        let world = capture_world(&msgs).expect("a file list");
        let line = tdm_room_line(&rom, &world).expect("a TDM room line for the map");
        let w = load_from_rom(&rom, &world, NameForm::Listed, None).expect("the capture's world");
        let (tables, _) = RomTables::load(&rom);
        let dc_dserver::pipe::Control::Room(room) = dc_dserver::pipe::parse_control(&line) else {
            panic!("room line");
        };
        let key = room.mode_key().to_ascii_uppercase().into_bytes();
        let in_mode = |i: usize| {
            w.spawn_modes[i].as_deref().is_some_and(|m| {
                m.iter()
                    .copied()
                    .filter(u8::is_ascii_uppercase)
                    .collect::<Vec<u8>>()
                    == key
            })
        };
        let team_of = |i: usize| tables.teams.team_of(w.spawns[i].team_filter.as_deref());
        let mut g = Game::new(GameTuning::default(), &w, tables.clone());
        g.set_room(&room);
        let mut other_mode = 0;
        for team in [1u8, 2] {
            let mine: Vec<usize> = (0..w.spawns.len())
                .filter(|&i| team_of(i) == Some(team))
                .collect();
            other_mode += mine.iter().filter(|&&i| !in_mode(i)).count();
            assert!(
                mine.iter().any(|&i| in_mode(i)),
                "{label}: team {team} has TDM points"
            );
            for _ in 0..300 {
                let (pos, _) = g.choose_spawn(team);
                let i = w
                    .spawns
                    .iter()
                    .position(|s| s.pos == pos)
                    .expect("a spawn point");
                assert!(
                    team_of(i) == Some(team) && in_mode(i),
                    "{label}: team {team} spawned on point {i} of another team or mode"
                );
            }
        }
        assert!(
            other_mode > 0,
            "{label}: the world has team points of other modes"
        );
        let mut on_points = 0;
        for conn in [5, 6] {
            for (p, _) in reference_teleports(&link_messages(&cap, conn)) {
                let Some(i) = w
                    .spawns
                    .iter()
                    .position(|s| (s.pos[0] - p[0]).abs() < 0.5 && (s.pos[2] - p[2]).abs() < 0.5)
                else {
                    continue;
                };
                assert!(
                    in_mode(i),
                    "{label}: reference teleport {p:?} on point {i} of another mode"
                );
                on_points += 1;
            }
        }
        assert!(on_points > 0, "{label}: reference teleports on spawn points");
        println!(
            "{label}: 600 choices on the team's {} points only ({other_mode} team points of other modes never \
             chosen); {on_points} reference spawn teleports, all on points of the mode",
            String::from_utf8_lossy(&key)
        );
        checked += 1;
    }
    if checked == 0 {
        println!("skipped: no capture");
    }
}

/// The records of one object in an object group body (the bits after the id byte), each with its wire `cf`.
fn records_of(body: &BitWriter, obj: u16) -> Vec<(u16, BitWriter)> {
    let mut out = Vec::new();
    let mut r = body.reader();
    while r.remaining_bits() >= 32 {
        let n = r.read_u32() as usize;
        if n > r.remaining_bits() {
            break;
        }
        let rec = r.read_bit_string(n);
        let mut h = rec.reader();
        let lo = h.read_u8();
        if lo == 0 {
            continue;
        }
        let cf = if lo & 0x80 != 0 {
            u16::from(lo) | (u16::from(h.read_u8()) << 8)
        } else {
            u16::from(lo)
        };
        if h.read_u16() == obj {
            out.push((cf, rec));
        }
    }
    out
}

/// DSV-44 amendment against the reference stack's death sequences on the real client's link (tdm4 bot 408, tdm6 bot 252):
/// (1) from the death to the respawn the dead character's stream carries the corpse's clip on the main tracker,
/// `state1` 0, its time the time since the death (within the tap's jitter) and held at the clip's end, the body
/// trackers still streamed (our `stream_for` rule, unit-tested in game.rs and game_sim.rs); (2) the respawn change
/// record our writer builds from the same values (models, trackers, flags, position, attachment) is the reference stack's
/// record bit for bit (`cf` 0x2ba8; tdm6 seq 10964).
// OBSERVED: capture 20261025_002236_tdm4 (seq 3132 .. 3510), 20261025_065016_tdm6 (seq 10134 ..
// 10964); aa-triage-red-dot §1.1, §1.2 (REFERENCE, accepted by the real client)
// DC-DEFAULT: DSV-44 (amendment, proposed)
#[test]
fn rom_death_sequences_match_the_reference() {
    use dc_botclient::objects::{ExtentBox, ObjectTable};
    use dc_dserver::modelobj::{character_respawned, ModelEntry, Tracker};
    use dc_dserver::objects::PosBox;
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let mut checked = 0;
    for (label, folder, victim) in [
        ("tdm4", "20261025_002236_tdm4", 408u16),
        ("tdm6", "20261025_065016_tdm6", 252u16),
    ] {
        let Some(cap) = capture_named(folder) else {
            println!("skipped: no {label} capture");
            continue;
        };
        let msgs = link_messages(&cap, 6);
        let world = capture_world(&msgs).expect("a file list");
        let w = load_from_rom(&rom, &world, NameForm::Listed, None).expect("the capture's world");
        let pbox = PosBox {
            min: w.extent_min,
            max: w.extent_max,
        };
        let mut t = ObjectTable {
            extents: Some(ExtentBox {
                min: w.extent_min,
                max: w.extent_max,
            }),
            ..ObjectTable::default()
        };
        let mut death: Option<(f64, u16)> = None;
        let (mut dead_entries, mut held) = (0, 0);
        let mut hold_value = 0;
        let mut respawn_equal = None;
        for (time, m) in &msgs {
            let mut r = m.reader();
            let id = r.read_u8();
            let n = r.remaining_bits();
            let body = r.read_bit_string(n);
            match id {
                8 => {
                    let before: Vec<u16> = t.objects.keys().copied().collect();
                    let was = t.objects.get(&victim).map(|o| o.flags);
                    t.apply_group(&body);
                    let now = t.objects.get(&victim).map(|o| o.flags);
                    if was != now && now == Some(0x0802) && death.is_none() {
                        let corpse = t
                            .objects
                            .iter()
                            .find(|(k, o)| !before.contains(k) && o.blob_type == Some(0x26))
                            .map(|(_, o)| o.anim[0].anim)
                            .expect("a corpse in the death group");
                        death = Some((*time, corpse));
                    }
                    if was == Some(0x0802) && now == Some(0x3803) && death.is_some() {
                        let o = &t.objects[&victim];
                        let recs = records_of(&body, victim);
                        let (cf, theirs) = recs.last().cloned().expect("the respawn record");
                        assert_eq!(cf, 0x2ba8, "{label}: the reference stack's respawn mask");
                        let models: Vec<ModelEntry> = o
                            .models
                            .iter()
                            .map(|e| ModelEntry {
                                file: e.model,
                                subs: e.subs.clone(),
                                textures: e.textures.clone(),
                                styles: e.styles.clone(),
                            })
                            .collect();
                        let pos = o.pos.expect("a position");
                        let weapon = o.attachments.first().map(|a| (a.child, a.socket));
                        // the time field width follows the clip length: each width is tried per tracker
                        let lens = [0u32, 100, 1000, 9000];
                        let mut equal = false;
                        for l0 in lens {
                            for l1 in lens {
                                for l2 in lens {
                                    let tr: Vec<Tracker> = o
                                        .anim
                                        .iter()
                                        .zip([l0, l1, l2])
                                        .map(|(a, len_ms)| Tracker {
                                            anim: a.anim,
                                            state0: a.state0,
                                            state1: a.state1,
                                            time_ms: a.time_ms,
                                            len_ms,
                                            secondary: a
                                                .tracker
                                                .map(|k| (k, a.blend_set.unwrap_or(0))),
                                        })
                                        .collect();
                                    let ours = character_respawned(
                                        victim,
                                        pos,
                                        models.clone(),
                                        tr,
                                        weapon,
                                    )
                                    .write(&pbox);
                                    equal |= ours.bit_len() == theirs.bit_len()
                                        && ours.as_bytes() == theirs.as_bytes();
                                }
                            }
                        }
                        respawn_equal = Some(equal);
                        break;
                    }
                }
                10 => {
                    let updates = t.objects.get(&victim).map_or(0, |o| o.anim_updates);
                    t.apply_stream(&body);
                    let Some((t0, clip)) = death else { continue };
                    let o = &t.objects[&victim];
                    if o.anim_updates == updates || o.flags != 0x0802 {
                        continue;
                    }
                    let main = o.anim[0];
                    assert_eq!(
                        (main.anim, main.state0, main.state1),
                        (clip, true, false),
                        "{label}: dead character at {time}: the corpse's clip, not looping"
                    );
                    assert_eq!(o.anim.len(), 3, "{label}: the body trackers still streamed");
                    let elapsed = ((time - t0) * 1000.0) as i64;
                    if main.time_ms > hold_value {
                        hold_value = main.time_ms;
                        assert!(
                            (i64::from(main.time_ms) - elapsed).abs() <= 120,
                            "{label}: time {} at {elapsed} ms after the death",
                            main.time_ms
                        );
                    } else {
                        held += 1;
                    }
                    dead_entries += 1;
                }
                _ => {}
            }
        }
        assert!(death.is_some(), "{label}: the victim's death");
        assert!(
            dead_entries >= 50 && held >= 20,
            "{label}: {dead_entries} entries, {held} held"
        );
        if label == "tdm6" {
            assert_eq!(
                respawn_equal,
                Some(true),
                "{label}: our respawn record = the reference stack's bit for bit"
            );
        }
        println!(
            "{label}: victim {victim}: {dead_entries} dead entries on the corpse's clip {} (time held at {hold_value} ms \
             in {held}); respawn record {}",
            death.map_or(0, |d| d.1),
            match respawn_equal {
                Some(true) => "equal to the reference stack's bit for bit",
                Some(false) => "DIFFERENT",
                None => "not in the capture",
            }
        );
        checked += 1;
    }
    if checked == 0 {
        println!("skipped: no capture");
    }
}

/// DSV-29 amendment: on the real client's link the reference stack's shot effects (id 17, type 0x06) on a player character
/// carry the ROM body surface (6) or, once in tdm6 (the head-shot kill), the head surface (4): our constants.
// OBSERVED: capture 20261025_002236_tdm4, 20261025_065016_tdm6 (REFERENCE, accepted by the real
// client); aa-triage-red-dot §1.3
#[test]
fn reference_character_hits_carry_the_body_surfaces() {
    let mut checked = 0;
    for (label, folder, chars, heads) in [
        ("tdm4", "20261025_002236_tdm4", &[408u16, 189][..], 0),
        ("tdm6", "20261025_065016_tdm6", &[252u16, 140][..], 1),
    ] {
        let Some(cap) = capture_named(folder) else {
            println!("skipped: no {label} capture");
            continue;
        };
        let (mut body, mut head) = (0, 0);
        for (_, m) in link_messages(&cap, 6) {
            let b = m.as_bytes();
            if b.len() < 11 || b[0] != 17 || b[1] != dc_dserver::gs::FX_SHOT {
                continue;
            }
            let first = u16::from_le_bytes([b[2], b[3]]);
            if !chars.contains(&first) {
                continue;
            }
            match b[10] {
                dc_dserver::gs::SURFACE_BODY => body += 1,
                dc_dserver::gs::SURFACE_HEAD => head += 1,
                a => panic!("{label}: a hit on character {first} with surface {a}"),
            }
        }
        assert!(body >= 4, "{label}: {body} body hits");
        assert_eq!(head, heads, "{label}: head hits");
        println!("{label}: {body} body hits with surface 6, {head} head hits with surface 4");
        checked += 1;
    }
    if checked == 0 {
        println!("skipped: no capture");
    }
}

/// HS-2: our HiSec case create equals the reference stack's case creates on the real client's link of tdm6 (six cases) and
/// tdm4 (one) bit for bit, given the reference stack's object id, case key, file ids, position and rotation; and the model,
/// skin and render style of the ROM case table's row 0 are the three files the reference stack announced for them.
#[test]
fn rom_case_create_matches_the_reference() {
    use dc_dserver::modelobj::{case_create, CaseSpec, ModelEntry};
    use dc_dserver::objects::PosBox;
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let (tables, _) = dc_dserver::rom_tables::RomTables::load(&rom);
    let row = tables.hisec.rows.get(&0).expect("case table row 0").clone();
    let spell = |s: &str| dc_dserver::chars::announce_spelling(s).into_bytes();
    let mut checked = 0;
    for (label, folder) in CAPTURES {
        let Some(cap) = capture_named(folder) else {
            println!("skipped: no {label} capture");
            continue;
        };
        let msgs = link_messages(&cap, 6);
        let world = capture_world(&msgs).expect("world");
        let w = load_from_rom(&rom, &world, NameForm::Listed, None).expect("the capture's world");
        let pbox = PosBox {
            min: w.extent_min,
            max: w.extent_max,
        };
        let mut files = std::collections::BTreeMap::new();
        for (_, m) in &msgs {
            let mut r = m.reader();
            match r.read_u8() {
                0x32 => {
                    while r.remaining_bits() >= 56 {
                        let id = r.read_u16();
                        r.read_u32();
                        files.insert(id, r.read_zstr8());
                    }
                }
                8 => {
                    while r.remaining_bits() >= 32 {
                        let n = r.read_u32() as usize;
                        if n > r.remaining_bits() {
                            break;
                        }
                        let rec = r.read_bit_string(n);
                        let mut q = rec.reader();
                        let lo = q.read_u8();
                        if lo & 0x80 == 0 || lo & 1 == 0 {
                            continue;
                        }
                        q.read_u8();
                        let obj = q.read_u16();
                        if q.read_u8() != 0x41 {
                            continue;
                        }
                        q.skip_bits(96);
                        let bl = q.read_u8() as usize;
                        let blob = q.read_bit_string(bl);
                        let mut b = blob.reader();
                        if bl != 83 || b.read_u8() != 0x17 {
                            continue;
                        }
                        b.skip_bits(3 + 8);
                        if b.read_u32() != 5 {
                            continue;
                        }
                        let key = b.read_u32();
                        // the model block: one model, its sub count, one skin, one style
                        assert_eq!(q.read_bits(6), 1);
                        let file = q.read_u16();
                        assert_eq!(q.read_bits(6), 1);
                        assert_eq!(q.read_bits(6), 1);
                        let tex = q.read_u16();
                        assert_eq!(q.read_bits(4), 1);
                        let style = q.read_u16();
                        assert_eq!(
                            files[&file],
                            spell(row.model.as_deref().unwrap()),
                            "{label}: model"
                        );
                        assert_eq!(files[&tex], spell(&row.skins[0]), "{label}: skin");
                        assert_eq!(
                            files[&style],
                            spell(&row.styles[0]),
                            "{label}: render style"
                        );
                        // past the tracker (24 bits), the flag words (96) and the scale (96)
                        q.skip_bits(24 + 96 + 96);
                        let p48 = dc_wire::udp::Pos48::read(&mut q);
                        q.skip_bits(72);
                        let rot = dc_wire::udp::read_quat128(&mut q);
                        let pos = dc_dserver::stream::pos48_decode(p48, &pbox);
                        let ours = case_create(&CaseSpec {
                            obj,
                            case_key: key,
                            model: ModelEntry {
                                file,
                                subs: Vec::new(),
                                textures: vec![tex],
                                styles: vec![style],
                            },
                            scale: row.scale.unwrap_or([1.0; 3]),
                            pos,
                            rot,
                        })
                        .write(&pbox);
                        assert_eq!(ours.bit_len(), rec.bit_len(), "{label}: case {key} length");
                        assert_eq!(ours.as_bytes(), rec.as_bytes(), "{label}: case {key} bits");
                        checked += 1;
                    }
                }
                _ => {}
            }
        }
    }
    if checked > 0 {
        assert_eq!(checked, 7, "six tdm6 cases and one tdm4 case");
    }
    println!("{checked} reference case creates reproduced");
}
/// DSV-80: the ROM's default attachments as part bytes. The L96A1 (weapon 27) gets its scope mod 27 in part 1; the
/// reference stack's grants on the real client's link of tdm6 / tdm4 (weapons without defaults) are rebuilt bit for bit with
/// the parts of our rule; our rust6 grants of weapon 27 change in part 1 only.
// DISASM: cshell+0x60d90 (0xb2 / 0xb3), cshell+0x1f4180 (zoom needs the socket-1 mod)
// OBSERVED: capture 20261025_065016_tdm6 c6 seq 733 (REFERENCE: -1 x 4 for 145 / 164 / 0 / 7 /
// 125); capture 20261025_125432_rust6 c5 seq 810 (ours: weapon 27 with -1 x 4)
// DC-DEFAULT: DSV-80 (proposed)
#[test]
fn rom_weapon_grants_carry_the_default_attachments() {
    use dc_dserver::gs::{drawn_weapon, weapon_give, WeaponSpec, NO_PART, NO_PARTS};
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let (tables, _) = dc_dserver::rom_tables::RomTables::load(&rom);
    let w = &tables.weapons;
    assert_eq!(
        w.default_parts(27),
        [27, NO_PART, NO_PART, NO_PART],
        "L96A1 scope"
    );
    for k in [145u16, 164, 0, 7, 125] {
        assert_eq!(w.default_parts(k), NO_PARTS, "weapon {k} has no default");
    }
    let rebuild = |m: &BitWriter| -> Option<(u16, [i8; 4], BitWriter)> {
        let mut r = m.reader();
        let shell = match (r.read_u8(), r.read_u8()) {
            (0x0d, s @ (0xb2 | 0xb3)) => s,
            _ => return None,
        };
        let weapon = r.read_u16();
        let ammo_kind = r.read_u16();
        let ammo = r.read_f32();
        let clip = if shell == 0xb3 { r.read_u32() } else { 0 };
        let class_pos = r.read_u8();
        let slot = r.read_u8();
        let sent = [r.read_s8(), r.read_s8(), r.read_s8(), r.read_s8()];
        let s = WeaponSpec {
            weapon,
            ammo_kind,
            ammo,
            clip,
            class_pos,
            slot,
            parts: w.default_parts(weapon),
        };
        let ours = if shell == 0xb2 {
            weapon_give(&s)
        } else {
            drawn_weapon(&s)
        };
        Some((weapon, sent, ours))
    };
    let mut reference = 0;
    for (label, folder) in CAPTURES {
        let Some(cap) = capture_named(folder) else {
            println!("skipped: no {label} capture");
            continue;
        };
        for (t, m) in link_messages(&cap, 6) {
            if let Some((weapon, _, ours)) = rebuild(&m) {
                assert_eq!(
                    ours.bit_len(),
                    m.bit_len(),
                    "{label} t {t}: weapon {weapon}"
                );
                assert_eq!(
                    ours.as_bytes(),
                    m.as_bytes(),
                    "{label} t {t}: weapon {weapon}"
                );
                reference += 1;
            }
        }
    }
    let mut ours27 = 0;
    if let Some(cap) = capture_named("20261025_125432_rust6") {
        for (t, m) in link_messages(&cap, 5) {
            let Some((weapon, sent, ours)) = rebuild(&m) else {
                continue;
            };
            if weapon != 27 {
                continue;
            }
            assert_eq!(sent, NO_PARTS, "rust6 t {t}: the old grant");
            let (a, b) = (m.as_bytes(), ours.as_bytes());
            assert_eq!(a.len(), b.len());
            let diff: Vec<usize> = (0..a.len()).filter(|&i| a[i] != b[i]).collect();
            assert_eq!(diff, vec![a.len() - 4], "rust6 t {t}: only part 1 changes");
            assert_eq!(b[a.len() - 4], 0x1b);
            ours27 += 1;
        }
    }
    println!("{reference} reference grants rebuilt bit for bit, {ours27} rust6 weapon-27 grants now carry the scope");
}

/// DSV-85 / DSV-86: every shot effect record the reference stack sent on the user's link (character hits, continuations,
/// misses, blasts) rebuilds bit for bit from its own fields through our writer, so the writer's layout is the
/// reference stack's; `c` carries the shooter's client id (the game_sim tests cover the value rule). Capture bytes are read
/// at run time, never committed (REFERENCE_TESTING.md section 5).
// OBSERVED: capture 20261025_002236_tdm4 c6 (seq 2968 and its continuation),
// 20261025_065016_tdm6 c6 (seq 6603, 2942, ...); REFERENCE
#[test]
fn reference_shot_effect_records_rebuild_bit_for_bit() {
    use dc_dserver::gs::{shot_effect, ShotEffect};
    let mut checked = 0;
    for (label, folder) in CAPTURES {
        let Some(cap) = capture_named(folder) else {
            println!("skipped: no {label} capture");
            continue;
        };
        for (_, m) in link_messages(&cap, 6) {
            let b = m.as_bytes();
            if b.len() != 54 || b[0] != 0x11 || b[1] != 0x06 {
                continue;
            }
            let u16_at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
            let v3_at = |o: usize| {
                let f = |k: usize| f32::from_le_bytes([b[k], b[k + 1], b[k + 2], b[k + 3]]);
                [f(o), f(o + 4), f(o + 8)]
            };
            let first = u16_at(2);
            let e = shot_effect(&ShotEffect {
                first: if first == 0xffff { None } else { Some(first) },
                shooter: u16_at(4),
                weapon: u16_at(6),
                ammo: u16_at(8),
                surface: b[10],
                mask: u16_at(11),
                client: b[13],
                mods: [b[51], b[52], b[53]],
                from: v3_at(14),
                to: v3_at(26),
                normal: v3_at(38),
            });
            if e.as_bytes() != b {
                // records with a trailer our writer does not model are reported, not asserted blindly
                panic!("{label}: a shot effect record does not rebuild: {:02x?}", b);
            }
            assert_eq!(e.bit_len(), 432, "{label}");
            checked += 1;
        }
    }
    println!("{checked} reference shot effect records rebuilt");
    assert!(checked == 0 || checked > 100, "too few records: {checked}");
}
/// V-SURF-1 (DSV-62): the reference stack's shot effect surface byte of every world hit (`first` 0) equals the surface word
/// of the world polygon at the record's end point, and that of every blast (no hit object, a throw's ids) the
/// surface word of the polygon below the blast point, on JUNK_FLEA (tdm6: `a` 10 / 20) and SNOWVALLEY (tdm4: `a`
/// 20 / 71; tdm2: `a` 10 / 20 / 71). The records with weapon 0 (`first` 0xFFFF, `a` 110, meaning not traced) are
/// not shots and are skipped.
#[test]
fn rom_world_surfaces_match_the_reference_shot_records() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let f32at = |b: &[u8], o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let vec = |b: &[u8], o: usize| [f32at(b, o), f32at(b, o + 4), f32at(b, o + 8)];
    let mut checked = 0;
    for (label, folder, min_world, min_blasts) in [
        ("tdm6", "20261025_065016_tdm6", 200, 2),
        ("tdm4", "20261025_002236_tdm4", 4, 1),
        ("tdm2", "20261025_001409_tdm2", 8, 1),
    ] {
        let Some(cap) = capture_named(folder) else {
            println!("skipped: no {label} capture");
            continue;
        };
        let msgs = link_messages(&cap, 6);
        let world = capture_world(&msgs).expect("world");
        let w = load_from_rom(&rom, &world, NameForm::Listed, None).expect("the capture's world");
        let probe = w.surfaces.as_ref().expect("surface probe");
        let (mut hits, mut blasts) = (0, 0);
        let mut seen = std::collections::BTreeSet::new();
        for (_, m) in &msgs {
            let b = m.as_bytes();
            if b.len() < 50 || b[0] != 17 || b[1] != dc_dserver::gs::FX_SHOT {
                continue;
            }
            let first = u16::from_le_bytes([b[2], b[3]]);
            let weapon = u16::from_le_bytes([b[6], b[7]]);
            let a = b[10];
            let (from, to) = (vec(b, 14), vec(b, 26));
            if first == 0 {
                let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
                let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                let dir = [d[0] / l, d[1] / l, d[2] / l];
                assert_eq!(
                    probe.at_shot_end(to, dir),
                    Some(a),
                    "{label}: world hit at {to:?}"
                );
                hits += 1;
            } else if first == 0xFFFF && weapon != 0 {
                assert_eq!(probe.below(to), Some(a), "{label}: blast at {to:?}");
                blasts += 1;
            } else {
                continue;
            }
            seen.insert(a);
        }
        assert!(hits >= min_world, "{label}: {hits} world hits");
        assert!(blasts >= min_blasts, "{label}: {blasts} blasts");
        println!(
            "{label}: {hits} world hits and {blasts} blasts equal the polygon surfaces {seen:?}"
        );
        checked += 1;
    }
    if checked == 0 {
        println!("skipped: no capture");
    }
}

/// DSV-64 against the ROM and the reference stack's event sounds: the ROM tables give pain variants and a head-shot sound;
/// every surface word of the captured worlds' polygons is a row `Id` of the surface table; each of the reference stack's
/// grenade bounce sounds (prio 1) plays a surface row's grenade impact sound (how often it is the one of the polygon
/// nearest to the sound's point is printed); the reference stack's pain and head-shot records carry our form (prio 2, outer 500,
/// ratio 17), and every pain file the reference stack used is either one of our variants or another file of the same folder.
#[test]
fn rom_event_sounds_follow_the_reference() {
    use dc_dserver::objects::PosBox;
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let (tables, _) = dc_dserver::rom_tables::RomTables::load(&rom);
    let ev = &tables.event_sounds;
    assert!(!ev.pain.is_empty(), "pain variants");
    assert!(ev.headshot.is_some(), "head-shot sound");
    assert!(!ev.grenade_impact.is_empty(), "grenade impact sounds");
    let up = |s: &str| s.replace('/', "\\").to_ascii_uppercase();
    let folder = |s: &str| {
        let u = up(s);
        u.rsplit_once('\\')
            .map(|x| x.0.to_string())
            .unwrap_or_default()
    };
    let mut checked = 0;
    for (label, folder_name) in [
        ("tdm6", "20261025_065016_tdm6"),
        ("tdm4", "20261025_002236_tdm4"),
        ("tdm1", "20261025_000014_tdm1"),
    ] {
        let Some(cap) = capture_named(folder_name) else {
            println!("skipped: no {label} capture");
            continue;
        };
        let conn = if label == "tdm1" { 5 } else { 6 };
        let msgs = link_messages(&cap, conn);
        let world = capture_world(&msgs).expect("world");
        let w = load_from_rom(&rom, &world, NameForm::Listed, None).expect("the capture's world");
        let probe = w.surfaces.as_ref().expect("surface probe");
        let pbox = PosBox {
            min: w.extent_min,
            max: w.extent_max,
        };
        let mut files = std::collections::BTreeMap::new();
        let (mut bounces, mut pains, mut nearest_agrees) = (0, 0, 0);
        for (t, m) in &msgs {
            let mut r = m.reader();
            match r.read_u8() {
                0x32 => {
                    while r.remaining_bits() >= 56 {
                        let id = r.read_u16();
                        r.read_u32();
                        files.insert(id, String::from_utf8_lossy(&r.read_zstr8()).into_owned());
                    }
                }
                8 => {
                    while r.remaining_bits() >= 32 {
                        let n = r.read_u32() as usize;
                        if n > r.remaining_bits() {
                            break;
                        }
                        let rec = r.read_bit_string(n);
                        let mut q = rec.reader();
                        // a started tracked sound with every cached field (the first use of its file)
                        if n != 192 || q.read_u8() != 0 || q.read_u8() != 1 || q.read_u8() != 3 {
                            continue;
                        }
                        q.read_u16();
                        let sfile = q.read_u16();
                        if q.read_u8() != 7 {
                            continue;
                        }
                        let sflags = q.read_u16();
                        let prio = q.read_u8();
                        let outer = q.read_u16();
                        let ratio = q.read_u8();
                        q.read_u32();
                        let pos = dc_dserver::stream::pos48_decode(
                            dc_wire::udp::Pos48::read(&mut q),
                            &pbox,
                        );
                        let Some(name) = files.get(&sfile) else {
                            continue;
                        };
                        // the world sounds have other flags (looping, ambient)
                        if sflags != 0x132 {
                            continue;
                        }
                        assert_eq!(ratio, 17, "{label} t {t}: {name}");
                        if prio == 1 {
                            assert_eq!(outer, dc_dserver::sounds::BOUNCE_FORM.outer);
                            // the polygon nearest to the point along the six axes (the bounce surface: a floor, or
                            // a wall the object hit), the floor below it when none is within 16 units
                            let mut near: Option<(f32, u8)> = None;
                            for d in [
                                [0.0f32, -1.0, 0.0],
                                [1.0, 0.0, 0.0],
                                [-1.0, 0.0, 0.0],
                                [0.0, 0.0, 1.0],
                                [0.0, 0.0, -1.0],
                                [0.0, 1.0, 0.0],
                            ] {
                                for k in [1.0f32, 2.0, 4.0, 8.0, 16.0] {
                                    let to =
                                        [pos[0] + d[0] * k, pos[1] + d[1] * k, pos[2] + d[2] * k];
                                    if let Some(x) = probe.first_on(pos, to) {
                                        if near.is_none_or(|n| k < n.0) {
                                            near = Some((k, x));
                                        }
                                        break;
                                    }
                                }
                            }
                            // the file is a surface row's grenade impact sound; whether it is the one of the polygon
                            // nearest to the sound's point is counted (the point is the object's position in that
                            // tick, not the contact point, so it is evidence, not a rule check)
                            let up_name = up(name);
                            assert!(
                                ev.grenade_impact.values().any(|f| up(f) == up_name),
                                "{label} t {t}: {name} is no surface row's grenade impact sound"
                            );
                            let s = near.map(|n| n.1).or_else(|| probe.below(pos));
                            if s.and_then(|s| ev.grenade_impact.get(&u32::from(s)))
                                .is_some_and(|f| up(f) == up_name)
                            {
                                nearest_agrees += 1;
                            }
                            bounces += 1;
                        } else {
                            assert_eq!((prio, outer), (2, 500), "{label} t {t}: {name}");
                            let mine: Vec<String> = ev.pain.iter().map(|f| up(f)).collect();
                            let head = ev.headshot.as_deref().map(up);
                            assert!(
                                mine.contains(&up(name))
                                    || head.as_deref() == Some(up(name).as_str())
                                    || mine.iter().any(|f| folder(f) == folder(name)),
                                "{label} t {t}: {name} is not a pain or head-shot file of ours"
                            );
                            pains += 1;
                        }
                    }
                }
                _ => {}
            }
        }
        println!(
            "{label}: {bounces} first-use bounce sounds ({nearest_agrees} equal the nearest polygon's), {pains} pain or head-shot sounds"
        );
        assert!(bounces + pains > 0, "{label}: no event sound");
        checked += 1;
    }
    // every polygon surface word of the captured worlds names a surface row
    for world in ["JUNK_FLEA", "SNOWVALLEY"] {
        let bytes = {
            let arch =
                dc_rez::RezArchive::open(&rom.join("Game").join(dc_dserver::world::WORLD_ARCHIVE))
                    .unwrap();
            let listed = dc_dserver::world::find_entry(arch.entries(), world)
                .unwrap()
                .to_string();
            let raw = arch.entry_raw(&listed).unwrap().unwrap();
            if raw.starts_with(&85u32.to_le_bytes()) {
                raw
            } else {
                let key = dc_rez::dc_rom::crypt_key(&rom).unwrap();
                dc_rez::decrypt_with_key(&raw, &key).unwrap()
            }
        };
        let wd =
            dc_world::World::parse_with(&bytes, dc_world::ParseOptions { render: false }).unwrap();
        let mut words = std::collections::BTreeSet::new();
        for m in &wd.models {
            for p in &m.polygons {
                words.insert(u32::from(m.surfaces[p.surface as usize].texflags));
            }
        }
        let missing: Vec<u32> = words
            .iter()
            .copied()
            .filter(|w| !ev.surface_ids.contains(w))
            .collect();
        assert!(
            missing.is_empty(),
            "{world}: surface words without a row: {missing:?}"
        );
        println!(
            "{world}: {} surface words, all rows of the surface table",
            words.len()
        );
    }
    if checked == 0 {
        println!("skipped: no capture");
    }
}
/// DSV-99 / DSV-100 (ROM): every weapon option row of the item table (`[Option<n>]`, item `1000 + ItemID`) names the
/// mod `[Mod<ItemID>]`, whose `Kinds` is the option's socket (ItemType 0x11 / 0x12 / 0x13 -> sockets 0 / 1 / 2), so
/// `ItemID` is the part byte the grant carries. Values are read from the ROM at run time, never copied.
// EVIDENCE: cshell+0x25c010 (1000s = options, n % 1000 kept), cshell+0x176f10 (Kinds), cshell+0x235ec0 (socket = Kinds)
// DC-DEFAULT: DSV-99 (proposed)
#[test]
fn rom_option_items_name_the_mod_of_their_socket() {
    use dc_dserver::rom_tables::{item_no, ItemNo, ATTRIBUTES_REZ, ITEMS_ENTRY};
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let (tables, _) = dc_dserver::rom_tables::RomTables::load(&rom);
    let vfs = dc_rez::RezVfs::new(&rom);
    let doc = dc_bute::ButeDoc::try_from_vfs(&vfs, ATTRIBUTES_REZ, ITEMS_ENTRY)
        .expect("item table")
        .expect("item table entry");
    let mut checked = 0;
    for (tag, sec) in doc.iter() {
        if !tag.starts_with("Option") {
            continue;
        }
        let num = |k: &str| sec.get(k).and_then(|v| v.trim().parse::<u32>().ok());
        let (Some(id), Some(kind)) = (num("ItemID"), num("ItemType")) else {
            continue;
        };
        let ItemNo::Option(m) = item_no(1000 + id) else {
            panic!("{tag}: not an option number");
        };
        let row = tables
            .weapons
            .mods
            .get(&m)
            .unwrap_or_else(|| panic!("{tag}: no mod row {m}"));
        assert_eq!(
            row.socket.map(|s| u32::from(s) + 0x11),
            Some(kind),
            "{tag}: mod {m} socket vs option kind"
        );
        checked += 1;
    }
    assert!(checked > 0, "no option rows");
    println!("{checked} option rows name the mod of their socket");
}

/// DSV-99 (ROM + captures): the rust7 finding. The user attached a sight to the M4A1 in the lobby; the 2009 client's
/// own `UP_PLAYER_INFO` names it (entry 145, sight field = mod 35), our grants of weapon 145 on that link carried
/// -1 x 4 (the bug), and the grant rebuilt with the player's attachments differs in part 1 only, which then holds a
/// sight-socket mod with a zoom level. The reference stack captures (tdm6 / tdm4) have no real-client attachment: every option
/// field of the real client's entries is 0xff and every reference grant on its link carries -1 x 4.
// SPEC: GAME_MESSAGES_2009_C2S.md §5.1 (`e1..e3`); EVIDENCE: cshell+0x226700 (fields by kind 0x11 / 0x12 / 0x13)
// DISASM: cshell+0x60d90, cshell+0x235ec0, cshell+0x1f4180
// OBSERVED: capture 20261025_153705_rust7 (conn 5 = the user: 0x72 entry (145, ff, 35, ff); 14
// grants of 145 with -1 x 4); 20261025_065016_tdm6 / 20261025_002236_tdm4 conn 6 (REFERENCE, no attachments)
// DC-DEFAULT: DSV-99 (proposed)
#[test]
fn rom_rust7_m4a1_sight_reaches_the_grant() {
    use dc_dserver::gs::{weapon_give, WeaponSpec, NO_PARTS};
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let (tables, _) = dc_dserver::rom_tables::RomTables::load(&rom);
    let w = &tables.weapons;
    let player_info = |cap: &std::path::Path, conn: u32| {
        let d = dc_botclient::capdecode::decode_file(cap).unwrap();
        let found = d.of(conn, true).find(|m| m.shell() == Some(0x72)).map(|m| {
            let b = m.bits.as_bytes();
            dc_dserver::up::parse_player_info(&BitWriter::from_bytes(&b[2..]))
        });
        found.expect("a 0x72")
    };
    let grants = |cap: &std::path::Path, conn: i64| -> Vec<(u16, [i8; 4], BitWriter)> {
        link_messages(cap, conn)
            .into_iter()
            .filter_map(|(_, m)| {
                let mut r = m.reader();
                if (r.read_u8(), r.read_u8()) != (0x0d, 0xb2) {
                    return None;
                }
                let s = WeaponSpec {
                    weapon: r.read_u16(),
                    ammo_kind: r.read_u16(),
                    ammo: r.read_f32(),
                    clip: 0,
                    class_pos: r.read_u8(),
                    slot: r.read_u8(),
                    parts: [r.read_s8(), r.read_s8(), r.read_s8(), r.read_s8()],
                };
                Some((s.weapon, s.parts, m))
            })
            .collect()
    };
    for (label, folder) in CAPTURES {
        let Some(cap) = capture_named(folder) else {
            println!("skipped: no {label} capture");
            continue;
        };
        let info = player_info(&cap, 6);
        assert!(!info.loadout.is_empty(), "{label}");
        for e in &info.loadout {
            assert_eq!([e.1, e.2, e.3], [0xff; 3], "{label}: {e:?}");
        }
        let g = grants(&cap, 6);
        assert!(!g.is_empty(), "{label}");
        assert!(g.iter().all(|x| x.1 == NO_PARTS), "{label}");
    }
    let Some(cap) = capture_named("20261025_153705_rust7") else {
        println!("skipped: no rust7 capture");
        return;
    };
    let info = player_info(&cap, 5);
    let m4 = info
        .loadout
        .iter()
        .find(|e| e.0 == 145)
        .expect("the M4A1 entry");
    assert_eq!((m4.1, m4.3), (0xff, 0xff), "no muzzle, no magazine");
    let sight = u16::from(m4.2);
    assert_ne!(m4.2, 0xff, "the sight field is set");
    let parts = w.player_parts(145, [None, Some(sight), None]);
    assert_eq!(parts, [sight as i8, -1, -1, -1], "the sight in part 1");
    assert!(w.zooms_with(&parts), "the sight zooms");
    let ours = grants(&cap, 5);
    let m4_grants: Vec<_> = ours.iter().filter(|x| x.0 == 145).collect();
    assert!(!m4_grants.is_empty());
    for (_, sent, m) in &m4_grants {
        assert_eq!(*sent, NO_PARTS, "rust7: the grant carried no attachment");
        let mut r = m.reader();
        r.read_u8();
        r.read_u8();
        let s = WeaponSpec {
            weapon: r.read_u16(),
            ammo_kind: r.read_u16(),
            ammo: r.read_f32(),
            clip: 0,
            class_pos: r.read_u8(),
            slot: r.read_u8(),
            parts,
        };
        let (a, b) = (m.as_bytes(), weapon_give(&s));
        let b = b.as_bytes();
        assert_eq!(a.len(), b.len());
        let diff: Vec<usize> = (0..a.len()).filter(|&i| a[i] != b[i]).collect();
        assert_eq!(diff, vec![a.len() - 4], "only part 1 changes");
    }
    println!(
        "rust7: {} grants of the M4A1 now carry the sight in part 1",
        m4_grants.len()
    );
}
/// DSV-93: every weapon the reference stack dropped at a body (type-0x17 kind-6 pickups with the falling flag, on both links
/// of tdm6 and tdm4: 7 + 7 + 1 + 1 creates) rebuilds bit for bit through `weapon_drop_create` from its own fields
/// (object id, weapon, rounds, file ids, position, rotation), and its model, skin and render style are the files the
/// ROM weapon row names for the third-person model (the files `Game::weapon_look` picks). The reference stack's parked
/// pickups (falling flag clear, at -9999.9) are not rebuilt: DeCombat does not send them. Capture bytes are read at
/// run time, never committed (REFERENCE_TESTING.md section 5).
// OBSERVED: capture 20261025_065016_tdm6 c5 / c6, 20261025_002236_tdm4 c5 / c6 (REFERENCE,
// accepted by the real client)
// DC-DEFAULT: DSV-93 (proposed)
#[test]
fn reference_weapon_drop_records_rebuild_bit_for_bit() {
    use dc_dserver::modelobj::{weapon_drop_create, DropSpec, ModelEntry};
    use dc_dserver::objects::PosBox;
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let (tables, _) = dc_dserver::rom_tables::RomTables::load(&rom);
    let spell = |s: &str| dc_dserver::chars::announce_spelling(s).into_bytes();
    let mut checked = 0;
    let mut weapons = std::collections::BTreeSet::new();
    for (label, folder) in CAPTURES {
        let Some(cap) = capture_named(folder) else {
            println!("skipped: no {label} capture");
            continue;
        };
        for conn in [5, 6] {
            let msgs = link_messages(&cap, conn);
            let world = capture_world(&msgs).expect("world");
            let w =
                load_from_rom(&rom, &world, NameForm::Listed, None).expect("the capture's world");
            let pbox = PosBox {
                min: w.extent_min,
                max: w.extent_max,
            };
            let mut files = std::collections::BTreeMap::new();
            for (t, m) in &msgs {
                let mut r = m.reader();
                match r.read_u8() {
                    0x32 => {
                        while r.remaining_bits() >= 56 {
                            let id = r.read_u16();
                            r.read_u32();
                            files.insert(id, r.read_zstr8());
                        }
                    }
                    8 => {
                        while r.remaining_bits() >= 32 {
                            let n = r.read_u32() as usize;
                            if n > r.remaining_bits() {
                                break;
                            }
                            let rec = r.read_bit_string(n);
                            let mut q = rec.reader();
                            let lo = q.read_u8();
                            if lo & 0x80 == 0 || lo & 1 == 0 {
                                continue;
                            }
                            q.read_u8();
                            let obj = q.read_u16();
                            if q.read_u8() != 0x41 {
                                continue;
                            }
                            q.skip_bits(96);
                            let bl = q.read_u8() as usize;
                            let blob = q.read_bit_string(bl);
                            let mut b = blob.reader();
                            if bl != 83 || b.read_u8() != 0x17 {
                                continue;
                            }
                            let flags = (b.read_bit(), b.read_bit(), b.read_bit());
                            b.skip_bits(8);
                            if b.read_u32() != 6 || flags != (false, false, true) {
                                continue;
                            }
                            let (weapon, rounds) = (b.read_u16(), b.read_u16());
                            // the model block: one model, its sub count, one skin, one style
                            assert_eq!(q.read_bits(6), 1);
                            let file = q.read_u16();
                            assert_eq!(q.read_bits(6), 1);
                            assert_eq!(q.read_bits(6), 1);
                            let tex = q.read_u16();
                            assert_eq!(q.read_bits(4), 1);
                            let style = q.read_u16();
                            let row = tables
                                .weapons
                                .hand_model(weapon)
                                .expect("the weapon's third-person row");
                            assert_eq!(
                                files[&file],
                                spell(row.hh_model.as_deref().unwrap()),
                                "{label} c{conn} t {t}: model of weapon {weapon}"
                            );
                            assert_eq!(files[&tex], spell(&row.hh_skins[0]), "{label}: skin");
                            assert_eq!(files[&style], spell(&row.hh_styles[0]), "{label}: style");
                            // past the tracker (one narrow entry, tsel 0: 15 bits), the flag words (96) and the
                            // scale (96)
                            q.skip_bits(15 + 96 + 96);
                            let p48 = dc_wire::udp::Pos48::read(&mut q);
                            q.skip_bits(72);
                            let rot = dc_wire::udp::read_quat128(&mut q);
                            let pos = dc_dserver::stream::pos48_decode(p48, &pbox);
                            let ours = weapon_drop_create(&DropSpec {
                                obj,
                                weapon,
                                rounds,
                                model: ModelEntry {
                                    file,
                                    subs: Vec::new(),
                                    textures: vec![tex],
                                    styles: vec![style],
                                },
                                pos,
                                rot,
                            })
                            .write(&pbox);
                            assert_eq!(
                                ours.bit_len(),
                                rec.bit_len(),
                                "{label} c{conn} t {t}: drop {obj} length"
                            );
                            assert_eq!(
                                ours.as_bytes(),
                                rec.as_bytes(),
                                "{label} c{conn} t {t}: drop {obj} bits"
                            );
                            weapons.insert(weapon);
                            checked += 1;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    if checked > 0 {
        assert_eq!(
            checked, 16,
            "seven tdm6 drops and one tdm4 drop on two links each"
        );
    }
    println!("{checked} reference weapon drops rebuilt bit for bit (weapons {weapons:?})");
}

/// DSV-93: the drop's tracker plays animation 1 of the weapon's third-person model (the reference stack's form); every such
/// model of the ROM weapon table that the archive holds has at least two animations, so the index is in range for
/// any weapon that drops.
// OBSERVED: ROM table key WEAPONS.TXT [Weapon<n>] HHModel (the model files read at run time, nothing copied)
#[test]
fn rom_weapon_models_have_the_drop_animation() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let (tables, _) = dc_dserver::rom_tables::RomTables::load(&rom);
    let vfs = dc_rez::RezVfs::new(rom.clone());
    let (mut read, mut missing) = (0, 0);
    for (id, w) in &tables.weapons.weapons {
        let Some(m) = &w.hh_model else {
            continue;
        };
        let Some((rez, entry)) = dc_dserver::chars::archive_of(m) else {
            missing += 1;
            continue;
        };
        let Ok(Some(bytes)) = vfs.entry(&rez, &entry) else {
            missing += 1;
            continue;
        };
        let model = dc_model::Model::parse(&bytes).expect("a third-person weapon model");
        assert!(
            model.clips.len() > usize::from(dc_dserver::modelobj::TODO_DROP_ANIM),
            "weapon {id}: {} animations",
            model.clips.len()
        );
        read += 1;
    }
    assert!(read > 0, "the ROM names third-person weapon models");
    println!(
        "{read} third-person weapon models carry the drop animation ({missing} not in the archive)"
    );
}
